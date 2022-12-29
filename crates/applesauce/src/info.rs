use crate::{decmpfs, round_to_block_size, xattr};
use std::error::Error;
use std::ffi::{CStr, CString};
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::{fmt, io};
use walkdir::WalkDir;

pub use decmpfs::CompressionType;

pub struct DecmpfsInfo {
    pub compression_type: CompressionType,
    pub attribute_size: u64,
    pub orig_file_size: u64,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DecmpfsError {
    TooSmall,
    BadMagic,
}

impl fmt::Display for DecmpfsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match *self {
            DecmpfsError::TooSmall => "decmpfs xattr too small to hold compression header",
            DecmpfsError::BadMagic => "decmpfs xattr magic field has incorrect value",
        };
        f.write_str(s)
    }
}

impl Error for DecmpfsError {}

#[non_exhaustive]
pub struct AfscFileInfo {
    pub is_compressed: bool,
    pub on_disk_size: u64,
    pub stat_size: u64,

    pub xattr_count: u32,
    pub total_xattr_size: u64,

    pub resource_fork_size: Option<u64>,

    pub decmpfs_info: Option<Result<DecmpfsInfo, DecmpfsError>>,
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
    for entry in WalkDir::new(path) {
        let entry = entry?;
        let file_type = entry.file_type();

        #[allow(clippy::filetype_is_file)]
        if file_type.is_file() {
            let info = get(entry.path())?;
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
            let info = get_decmpfs_info(&path, xattr_name)?;
            decmpfs_info = Some(info);
        } else {
            let maybe_len = xattr::len(&path, xattr_name)?;
            let len = maybe_len.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "file claimed to have xattr '{}', but it has no len",
                        xattr_name.to_string_lossy()
                    ),
                )
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

fn get_decmpfs_info(
    path: &CString,
    xattr_name: &CStr,
) -> io::Result<Result<DecmpfsInfo, DecmpfsError>> {
    let maybe_contents = xattr::read(path, xattr_name)?;
    let contents = maybe_contents
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "cannot get decmpfs xattr"))?;
    if contents.len() < decmpfs::DiskHeader::SIZE {
        return Ok(Err(DecmpfsError::TooSmall));
    }
    let magic = &contents[..4];
    if magic != decmpfs::MAGIC {
        return Ok(Err(DecmpfsError::BadMagic));
    }
    let compression_type =
        CompressionType::from_raw_type(u32::from_le_bytes(contents[4..8].try_into().unwrap()));
    let uncompressed_size = u64::from_le_bytes(contents[8..16].try_into().unwrap());
    Ok(Ok(DecmpfsInfo {
        compression_type,
        attribute_size: contents.len().try_into().unwrap(),
        orig_file_size: uncompressed_size,
    }))
}
