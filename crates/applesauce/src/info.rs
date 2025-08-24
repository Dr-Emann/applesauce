use applesauce_core::{decmpfs, round_to_block_size};
use std::ffi::{CStr, CString};
use std::fmt;
use std::fs::Metadata;
use std::io;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use crate::volumes::Volumes;
use crate::xattr;
pub use applesauce_core::decmpfs::CompressionType;

pub struct DecmpfsInfo {
    pub compression_type: CompressionType,
    pub attribute_size: u64,
    pub orig_file_size: u64,
}

#[non_exhaustive]
pub struct AfscFileInfo {
    pub is_compressed: bool,
    pub on_disk_size: u64,
    pub stat_size: u64,

    pub xattr_count: u32,
    pub total_xattr_size: u64,

    pub resource_fork_size: Option<u64>,

    pub decmpfs_info: Option<Result<DecmpfsInfo, decmpfs::DecodeError>>,
}

#[non_exhaustive]
pub struct FileInfo {
    pub on_disk_size: u64,
    pub compression_state: FileCompressionState,
}

#[non_exhaustive]
pub enum IncompressibleReason {
    Empty,
    TooLarge(u64),
    IoError(io::Error),
    FsNotSupported,
    HasRequiredXattr,
}

impl fmt::Display for IncompressibleReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IncompressibleReason::Empty => write!(f, "empty file"),
            IncompressibleReason::TooLarge(size) => {
                write!(f, "file too large to compress: {size} bytes")
            }
            IncompressibleReason::IoError(e) => e.fmt(f),
            IncompressibleReason::FsNotSupported => {
                write!(f, "filesystem does not support compression")
            }
            IncompressibleReason::HasRequiredXattr => {
                write!(f, "file has a required xattr for compression already")
            }
        }
    }
}

pub enum FileCompressionState {
    Compressed,
    Compressible,
    Incompressible(IncompressibleReason),
}

impl AfscFileInfo {
    #[must_use]
    pub fn compressed_fraction(&self) -> f64 {
        self.on_disk_size as f64 / self.stat_size as f64
    }
}

#[derive(Debug, Default, Copy, Clone)]
#[non_exhaustive]
pub struct AfscFolderInfo {
    pub num_files: u32,
    pub num_folders: u32,
    pub num_compressed_files: u32,

    pub total_uncompressed_size: u64,
    pub total_compressed_size: u64,
}

impl AfscFolderInfo {
    #[must_use]
    pub fn compressed_fraction(&self) -> f64 {
        self.total_compressed_size as f64 / self.total_uncompressed_size as f64
    }

    #[must_use]
    pub fn compression_savings_fraction(&self) -> f64 {
        1.0 - self.compressed_fraction()
    }
}

pub fn get_recursive(path: &Path) -> io::Result<AfscFolderInfo> {
    let mut result = AfscFolderInfo::default();
    for entry in jwalk::WalkDir::new(path) {
        let entry = entry?;
        let file_type = entry.file_type();

        #[allow(clippy::filetype_is_file)]
        if file_type.is_file() {
            let info = get(&entry.path())?;
            result.num_files += 1;
            if info.is_compressed {
                result.num_compressed_files += 1;
                result.total_compressed_size += info.on_disk_size;
            } else {
                result.total_compressed_size += info.stat_size;
            }
            result.total_uncompressed_size += info.stat_size;
        } else if file_type.is_dir() {
            result.num_folders += 1;
        }
    }
    Ok(result)
}

pub fn get_file_info(path: &Path, metadata: &Metadata, volumes: &Volumes) -> FileInfo {
    let compression_info = get_compression_state(path, metadata, volumes);
    let on_disk_size = round_to_block_size(metadata.blocks() * 512, metadata.st_blksize());
    FileInfo {
        on_disk_size,
        compression_state: compression_info,
    }
}

#[tracing::instrument(level = "debug", skip_all)]
pub fn get_compression_state(
    path: &Path,
    metadata: &Metadata,
    volumes: &Volumes,
) -> FileCompressionState {
    if metadata.st_flags() & libc::UF_COMPRESSED != 0 {
        return FileCompressionState::Compressed;
    }

    if metadata.len() == 0 {
        return FileCompressionState::Incompressible(IncompressibleReason::Empty);
    }
    if metadata.len() >= u64::from(u32::MAX) {
        return FileCompressionState::Incompressible(IncompressibleReason::TooLarge(
            metadata.len(),
        ));
    }

    match volumes.supports_compression(path, metadata) {
        Ok(true) => {}
        Ok(false) => {
            return FileCompressionState::Incompressible(IncompressibleReason::FsNotSupported)
        }
        Err(e) => return FileCompressionState::Incompressible(IncompressibleReason::IoError(e)),
    };

    // TODO: Try a local buffer for non-alloc fast path
    let path = match CString::new(path.as_os_str().as_bytes()) {
        Ok(path) => path,
        Err(e) => {
            return FileCompressionState::Incompressible(IncompressibleReason::IoError(e.into()))
        }
    };
    match xattr::is_present(&path, resource_fork::XATTR_NAME) {
        Ok(true) => {
            return FileCompressionState::Incompressible(IncompressibleReason::HasRequiredXattr);
        }
        Ok(false) => {}
        Err(e) => {
            return FileCompressionState::Incompressible(IncompressibleReason::IoError(e));
        }
    };
    match xattr::is_present(&path, decmpfs::XATTR_NAME) {
        Ok(true) => {
            return FileCompressionState::Incompressible(IncompressibleReason::HasRequiredXattr);
        }
        Ok(false) => {}
        Err(e) => {
            return FileCompressionState::Incompressible(IncompressibleReason::IoError(e));
        }
    };

    FileCompressionState::Compressible
}

pub fn get(path: &Path) -> io::Result<AfscFileInfo> {
    let metadata = path.metadata()?;

    let on_disk_size = round_to_block_size(metadata.blocks() * 512, metadata.st_blksize());

    // TODO: Try a local buffer for non-alloc fast path
    let path = CString::new(path.as_os_str().as_bytes())?;

    let mut total_xattr_size = 0;
    let mut xattr_count = 0;
    let mut resource_fork_size = None;
    let mut decmpfs_info = None;
    xattr::with_names(&path, |xattr_name| {
        if xattr_name == decmpfs::XATTR_NAME {
            debug_assert!(decmpfs_info.is_none());
            let info = get_decmpfs_info(&path)?;
            decmpfs_info = Some(info);
        } else {
            let maybe_len = xattr::len(&path, xattr_name)?;
            let len = maybe_len.ok_or_else(|| {
                io::Error::other(format!(
                    "file claimed to have xattr '{}', but it has no len",
                    xattr_name.to_string_lossy()
                ))
            })?;
            let len = u64::try_from(len).unwrap();

            if xattr_name == resource_fork::XATTR_NAME {
                debug_assert!(resource_fork_size.is_none());
                resource_fork_size = Some(len);
            } else {
                xattr_count += 1;
                total_xattr_size += len;
            }
        }

        Ok(())
    })?;

    Ok(AfscFileInfo {
        is_compressed: (metadata.st_flags() & libc::UF_COMPRESSED) == libc::UF_COMPRESSED,
        on_disk_size,
        stat_size: metadata.len(),
        xattr_count,
        total_xattr_size,
        resource_fork_size,
        decmpfs_info,
    })
}

fn get_decmpfs_info(path: &CStr) -> io::Result<Result<DecmpfsInfo, decmpfs::DecodeError>> {
    let maybe_data = xattr::read(path, decmpfs::XATTR_NAME)?;
    let data = maybe_data.ok_or_else(|| io::Error::other("cannot get decmpfs xattr"))?;

    Ok(decmpfs_info_from_bytes(&data))
}

fn decmpfs_info_from_bytes(data: &[u8]) -> Result<DecmpfsInfo, decmpfs::DecodeError> {
    let value = decmpfs::Value::from_data(data)?;
    Ok(DecmpfsInfo {
        compression_type: value.compression_type,
        attribute_size: data.len().try_into().unwrap(),
        orig_file_size: value.uncompressed_size,
    })
}
