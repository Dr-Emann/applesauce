#![warn(unsafe_op_in_unsafe_fn)]
#![warn(clippy::undocumented_unsafe_blocks)]
#![warn(clippy::cast_lossless)]
#![warn(clippy::cast_ptr_alignment)]
#![warn(clippy::clone_on_ref_ptr)]
#![warn(clippy::cloned_instead_of_copied)]
#![warn(clippy::debug_assert_with_mut_call)]
#![warn(clippy::filetype_is_file)]
#![warn(clippy::match_same_arms)]

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
compile_error!("applesauce only works on macos/ios");

pub mod info;
pub mod progress;
pub use applesauce_core::compressor;

mod rfork_storage;
mod scan;
mod seq_queue;
mod threads;
mod xattr;

use libc::c_char;
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata};
use std::io::prelude::*;
use std::mem::MaybeUninit;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::{io, mem, ptr};
use tracing::warn;

use crate::progress::{Progress, SkipReason};
use crate::threads::{BackgroundThreads, Mode};
use applesauce_core::compressor::Kind;
use applesauce_core::decmpfs;

const fn c_char_bytes(chars: &[c_char]) -> &[u8] {
    assert!(mem::size_of::<c_char>() == mem::size_of::<u8>());
    assert!(mem::align_of::<c_char>() == mem::align_of::<u8>());
    // SAFETY: c_char is the same layout as u8
    unsafe { mem::transmute(chars) }
}

fn cstr_from_bytes_until_null(bytes: &[c_char]) -> Option<&CStr> {
    let bytes = c_char_bytes(bytes);
    let pos = memchr::memchr(0, bytes)?;
    CStr::from_bytes_with_nul(&bytes[..=pos]).ok()
}

const ZFS_SUBTYPE: u32 = u32::from_be_bytes(*b"ZFS\0");

#[tracing::instrument(level = "debug", skip_all)]
fn check_compressible(path: &Path, metadata: &Metadata) -> Result<(), SkipReason> {
    if !metadata.is_file() {
        return Err(SkipReason::NotFile);
    }
    if metadata.st_flags() & libc::UF_COMPRESSED != 0 {
        return Err(SkipReason::AlreadyCompressed);
    }
    if metadata.len() == 0 {
        return Err(SkipReason::EmptyFile);
    }
    if metadata.len() >= u64::from(u32::MAX) {
        return Err(SkipReason::TooLarge(metadata.len()));
    }

    // TODO: Try a local buffer for non-alloc fast path
    let path = match CString::new(path.as_os_str().as_bytes()) {
        Ok(path) => path,
        Err(e) => return Err(SkipReason::ReadError(e.into())),
    };

    let mut statfs_buf = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: path is a valid pointer, and null terminated, statfs_buf is a valid ptr, and is used as an out ptr
    let rc = unsafe { libc::statfs(path.as_ptr(), statfs_buf.as_mut_ptr()) };
    if rc != 0 {
        return Err(SkipReason::ReadError(io::Error::last_os_error()));
    }
    // SAFETY: if statfs returned non-zero, we returned already, it should have filled in statfs_buf
    let statfs_buf = unsafe { statfs_buf.assume_init_ref() };

    // TODO: let is_apfs = statfs_buf.f_fstypename.starts_with(APFS_CHARS);
    let is_zfs = statfs_buf.f_fssubtype == ZFS_SUBTYPE;

    if is_zfs {
        // ZFS doesn't do HFS/decmpfs compression. It may pretend to, but in
        // that case it will *de*compress the data before committing it. We
        // won't play that game, wasting cycles and rewriting data for nothing.
        return Err(SkipReason::ZfsFilesystem);
    }

    if xattr::is_present(&path, resource_fork::XATTR_NAME)?
        || xattr::is_present(&path, decmpfs::XATTR_NAME)?
    {
        return Err(SkipReason::HasRequiredXattr);
    }

    let root_path = cstr_from_bytes_until_null(&statfs_buf.f_mntonname)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "mount name invalid"))?;
    if !vol_supports_compression_cap(root_path)? {
        return Err(SkipReason::FsNotSupported);
    }
    Ok(())
}

#[tracing::instrument(level = "debug", skip_all)]
fn check_decompressible(metadata: &Metadata) -> Result<(), SkipReason> {
    if !metadata.is_file() {
        return Err(SkipReason::NotFile);
    }
    if metadata.st_flags() & libc::UF_COMPRESSED == 0 {
        return Err(SkipReason::NotCompressed);
    }
    Ok(())
}

fn vol_supports_compression_cap(mnt_root: &CStr) -> io::Result<bool> {
    #[repr(C)]
    struct VolAttrs {
        length: u32,
        vol_attrs: libc::vol_capabilities_attr_t,
    }
    const IDX: usize = libc::VOL_CAPABILITIES_FORMAT;
    const MASK: libc::attrgroup_t = libc::VOL_CAP_FMT_DECMPFS_COMPRESSION;

    // SAFETY: All fields are simple integers which can be zero-initialized
    let mut attrs = unsafe { MaybeUninit::<libc::attrlist>::zeroed().assume_init() };
    attrs.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    attrs.volattr = libc::ATTR_VOL_CAPABILITIES;

    let mut vol_attrs = MaybeUninit::<VolAttrs>::uninit();
    // SAFETY:
    // `mnt_root` is a valid pointer, and is null terminated
    // attrs is a valid pointer to initialized memory of the correct type
    // vol_attrs is a valid pointer, and its size is passed as the size of the buffer
    let rc = unsafe {
        libc::getattrlist(
            mnt_root.as_ptr(),
            ptr::addr_of_mut!(attrs).cast(),
            vol_attrs.as_mut_ptr().cast(),
            mem::size_of_val(&vol_attrs),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: getattrlist returned success
    let vol_attrs = unsafe { vol_attrs.assume_init_ref() };
    if vol_attrs.length != u32::try_from(mem::size_of::<VolAttrs>()).unwrap() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "getattrlist returned bad size",
        ));
    }

    Ok(vol_attrs.vol_attrs.valid[IDX] & vol_attrs.vol_attrs.capabilities[IDX] & MASK != 0)
}

#[tracing::instrument(level = "debug", skip_all)]
fn reset_times(file: &File, metadata: &Metadata) -> io::Result<()> {
    let times: [libc::timespec; 2] = [
        libc::timespec {
            tv_sec: metadata.atime(),
            tv_nsec: metadata.atime_nsec(),
        },
        libc::timespec {
            tv_sec: metadata.mtime(),
            tv_nsec: metadata.mtime_nsec(),
        },
    ];
    // SAFETY: fd is valid, times points to an array of 2 timespec values
    let rc = unsafe { libc::futimens(file.as_raw_fd(), times.as_ptr()) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[tracing::instrument(level = "trace", skip_all, fields(flags), err)]
fn set_flags(file: &File, flags: libc::c_uint) -> io::Result<()> {
    let rc =
        // SAFETY: fd is valid
        unsafe { libc::fchflags(file.as_raw_fd(), flags) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[derive(Default)]
pub struct FileCompressor {
    bg_threads: BackgroundThreads,
}

impl FileCompressor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[tracing::instrument(skip_all)]
    pub fn recursive_compress<'a, P>(
        &mut self,
        paths: impl IntoIterator<Item = &'a Path>,
        kind: Kind,
        minimum_compression_ratio: f64,
        level: u32,
        progress: &P,
    ) where
        P: Progress + Send + Sync,
        P::Task: Send + Sync + 'static,
    {
        self.bg_threads.scan(
            Mode::Compress {
                kind,
                level,
                minimum_compression_ratio,
            },
            paths,
            progress,
        );
    }

    #[tracing::instrument(skip_all)]
    pub fn recursive_decompress<'a, P>(
        &mut self,
        paths: impl IntoIterator<Item = &'a Path>,
        manual: bool,
        progress: &P,
    ) where
        P: Progress + Send + Sync,
        P::Task: Send + Sync + 'static,
    {
        let mode = if manual {
            Mode::DecompressManually
        } else {
            Mode::DecompressByReading
        };
        self.bg_threads.scan(mode, paths, progress);
    }
}

fn try_read_all<R: Read>(mut r: R, buf: &mut [u8]) -> io::Result<usize> {
    let bulk_read_span = tracing::trace_span!(
        "try_read_all",
        len = buf.len(),
        read_len = tracing::field::Empty,
    );
    let full_len = buf.len();
    let mut remaining = buf;
    loop {
        let _enter = bulk_read_span.enter();
        let n = match r.read(remaining) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if n == 0 {
            break;
        }
        remaining = &mut remaining[n..];
        if remaining.is_empty() {
            return Ok(full_len);
        }
    }
    let read_len = full_len - remaining.len();

    bulk_read_span.record("read_len", read_len);
    Ok(read_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::{Progress, Task};
    use applesauce_core::compressor;
    use std::{fs, iter};
    use tempfile::TempDir;
    use walkdir::WalkDir;

    struct NoProgress;
    impl Task for NoProgress {
        fn increment(&self, _amt: u64) {}
        fn error(&self, _message: &str) {}
    }
    impl Progress for NoProgress {
        type Task = NoProgress;

        fn error(&self, path: &Path, message: &str) {
            panic!("Expected no errors, got {message} for {path:?}");
        }

        fn file_task(&self, _path: &Path, _size: u64) -> Self::Task {
            NoProgress
        }
    }

    fn recursive_hash(dir: &Path) -> Vec<u8> {
        use sha2::Digest;
        let mut hasher = sha2::Sha512::new();

        for item in WalkDir::new(dir).sort_by_file_name() {
            let item = item.unwrap();
            if !item.file_type().is_dir() {
                hasher.update(item.path().as_os_str().as_bytes());
                hasher.update(fs::read(item.path()).unwrap());
            }
        }
        hasher.finalize().to_vec()
    }

    fn populate_dir(dir: &Path) {
        // Empty file
        fs::write(dir.join("EMPTY"), b"").unwrap();

        // Medium files
        for i in 0u8..=0xFF {
            let p = dir.join(format!("{i}"));
            fs::write(p, vec![i; usize::from(i) * 1024]).unwrap();
        }

        let subdir = dir.join("subdir");
        fs::create_dir(&subdir).unwrap();
        // Tiny Files
        for i in 0u8..=0xFF {
            let p = subdir.join(format!("{i}"));
            fs::write(p, vec![i; usize::from(i)]).unwrap();
        }

        let big_file = dir.join("BIG");
        let mut big_content = Vec::new();
        for i in 0u8..=0xFF {
            big_content.extend_from_slice(&[i; 1234]);
        }
        fs::write(big_file, big_content).unwrap();
    }

    fn compress_folder(compressor_kind: compressor::Kind, dir: &Path) {
        populate_dir(dir);
        let old_hash = recursive_hash(dir);

        let mut fc = FileCompressor::new();
        fc.recursive_compress(iter::once(dir), compressor_kind, 1.0, 2, &NoProgress);
        drop(fc);

        let new_hash = recursive_hash(dir);
        assert_eq!(old_hash, new_hash);

        let info = info::get_recursive(dir).unwrap();
        // These are very compressible files
        assert!(info.compression_savings_fraction() > 0.5);

        // Now Decompress
        let mut fc = FileCompressor::new();
        fc.recursive_decompress(iter::once(dir), true, &NoProgress);
        drop(fc);

        let new_hash = recursive_hash(dir);
        assert_eq!(old_hash, new_hash);
    }

    #[cfg(feature = "zlib")]
    #[test]
    fn compress_zlib() {
        let dir = TempDir::new().unwrap();
        compress_folder(compressor::Kind::Zlib, dir.path());
    }

    #[cfg(feature = "lzvn")]
    #[test]
    fn compress_lzvn() {
        let dir = TempDir::new().unwrap();
        compress_folder(compressor::Kind::Lzvn, dir.path());
    }

    #[cfg(feature = "lzfse")]
    #[test]
    fn compress_lzfse() {
        let dir = TempDir::new().unwrap();
        compress_folder(compressor::Kind::Lzfse, dir.path());
    }
}
