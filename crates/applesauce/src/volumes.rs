//! Per-volume information

use crate::cstr_from_bytes_until_null;
use dashmap::{DashMap, DashSet};
use std::ffi::CString;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::{fs, io};
use tempfile::{NamedTempFile, TempDir};

const TEMPDIR_PREFIX: &str = "applesauce_tmp";
const TEMPFILE_PREFIX: &str = "applesauce_tmp";
const ZFS_SUBTYPE: u32 = u32::from_be_bytes(*b"ZFS\0");

#[derive(Debug)]
enum VolumeInfo {
    NoCompression,
    SupportsCompression { temp_dir: Option<TempDir> },
}

#[derive(Debug)]
pub struct Volumes {
    infos: DashMap<u64, VolumeInfo>,
    tmp_dirs: DashSet<Box<Path>>,
}

impl Default for Volumes {
    fn default() -> Self {
        Self::new()
    }
}

fn vol_with_file_supports_compression(path: &Path) -> io::Result<bool> {
    let path_cstr = CString::new(path.as_os_str().as_bytes())?;
    let mut statfs_buf = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: path is a valid pointer, and null terminated, statfs_buf is a valid ptr, and is used as an out ptr
    let rc = unsafe { libc::statfs(path_cstr.as_ptr(), statfs_buf.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: if statfs returned non-zero, we returned already, it should have filled in statfs_buf
    let statfs_buf = unsafe { statfs_buf.assume_init_ref() };
    // TODO: let is_apfs = statfs_buf.f_fstypename.starts_with(APFS_CHARS);
    if statfs_buf.f_fssubtype == ZFS_SUBTYPE {
        return Ok(false);
    }
    let root_path = cstr_from_bytes_until_null(&statfs_buf.f_mntonname)
        .ok_or_else(|| io::Error::other("failed to get root path from statfs"))?;
    crate::vol_supports_compression_cap(root_path)
}

fn system_volinfo() -> Option<(u64, VolumeInfo)> {
    let system = match TempDir::with_prefix(TEMPDIR_PREFIX) {
        Ok(system) => system,
        Err(e) => {
            tracing::warn!("failed to create temp dir in system temp dir: {e}");
            return None;
        }
    };
    let system_metadata = match system.path().metadata() {
        Ok(system_metadata) => system_metadata,
        Err(e) => {
            tracing::warn!("failed to get metadata for system temp dir: {e}");
            return None;
        }
    };
    let compression_support = vol_with_file_supports_compression(system.path()).unwrap_or(false);
    let volume_info = if compression_support {
        VolumeInfo::SupportsCompression {
            temp_dir: Some(system),
        }
    } else {
        VolumeInfo::NoCompression
    };
    Some((system_metadata.dev(), volume_info))
}

impl Volumes {
    pub fn new() -> Self {
        let infos = DashMap::new();
        let tmp_dirs = DashSet::new();
        if let Some((dev, vol_info)) = system_volinfo() {
            if let VolumeInfo::SupportsCompression {
                temp_dir: Some(dir),
            } = &vol_info
            {
                tmp_dirs.insert(dir.path().into());
            }
            infos.insert(dev, vol_info);
        }
        Self { infos, tmp_dirs }
    }

    fn get_or_insert(
        &self,
        path: &Path,
        metadata: &fs::Metadata,
    ) -> io::Result<dashmap::mapref::one::Ref<'_, u64, VolumeInfo>> {
        let device = metadata.dev();
        // Double lookup, we don't need a write lock if the item is already present
        if let Some(vol_info) = self.infos.get(&device) {
            return Ok(vol_info);
        }
        self.infos
            .entry(device)
            .or_try_insert_with(|| {
                let tempdir_parent = if metadata.is_dir() {
                    path
                } else {
                    let parent = path
                        .parent()
                        .ok_or_else(|| io::Error::other("no parent of file on volume"))?;

                    if parent.metadata()?.dev() != device {
                        return Err(io::Error::other(
                            "file on volume has different device than parent",
                        ));
                    }

                    parent
                };
                let compression_support = vol_with_file_supports_compression(path)?;
                let volume_info = if compression_support {
                    let temp_dir = TempDir::with_prefix_in(TEMPDIR_PREFIX, tempdir_parent)?;
                    self.tmp_dirs.insert(temp_dir.path().into());
                    VolumeInfo::SupportsCompression {
                        temp_dir: Some(temp_dir),
                    }
                } else {
                    VolumeInfo::NoCompression
                };
                Ok(volume_info)
            })
            .map(|ref_mut| ref_mut.downgrade())
    }

    pub fn supports_compression(&self, path: &Path, metadata: &fs::Metadata) -> io::Result<bool> {
        let current = self.get_or_insert(path, metadata)?;
        Ok(matches!(
            current.value(),
            VolumeInfo::SupportsCompression { .. }
        ))
    }

    pub fn add_root_dir(&self, path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
        self.get_or_insert(path, metadata)?;
        Ok(())
    }

    pub fn is_temp_dir(&self, path: &Path) -> bool {
        self.tmp_dirs.contains(path)
    }

    pub fn tempfile_for(&self, path: &Path, metadata: &fs::Metadata) -> io::Result<NamedTempFile> {
        let current = self.get_or_insert(path, metadata)?;
        match current.value() {
            VolumeInfo::NoCompression => {
                Err(io::Error::other("volume does not support compression"))
            }
            VolumeInfo::SupportsCompression { temp_dir } => {
                let dir = match temp_dir {
                    Some(dir) => dir.path(),
                    None => return Err(io::Error::other("no temp dir for volume")),
                };
                let mut builder = tempfile::Builder::new();
                builder.prefix(TEMPFILE_PREFIX);
                if let Some(file_name) = path.file_name() {
                    builder.suffix(file_name);
                }
                builder.tempfile_in(dir)
            }
        }
    }
}
