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

pub mod compressor;
pub mod info;

mod decmpfs;
mod progress;
mod resource_fork;
mod seq_queue;
mod threads;
mod xattr;

use libc::c_char;
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata, Permissions};
use std::io::prelude::*;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::{fs, io, mem, ptr};

macro_rules! cstr {
    ($s:literal) => {{
        // TODO: Check for nulls
        // SAFETY: definitely null terminated, at worst terminated early
        unsafe { CStr::from_bytes_with_nul_unchecked(concat!($s, "\0").as_bytes()) }
    }};
}

use crate::threads::BackgroundThreads;
pub(crate) use cstr;

pub use progress::Progress;

const BLOCK_SIZE: usize = 0x10000;

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

const fn num_blocks(size: u64) -> u64 {
    (size + (BLOCK_SIZE as u64 - 1)) / (BLOCK_SIZE as u64)
}

#[tracing::instrument(level = "debug", skip_all)]
fn check_compressible(path: &Path, metadata: &Metadata) -> io::Result<()> {
    if !metadata.is_file() {
        return Err(io::Error::new(io::ErrorKind::Other, "not a file"));
    }
    if metadata.st_flags() & libc::UF_COMPRESSED != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "file already compressed",
        ));
    }
    if metadata.len() >= u64::from(u32::MAX) {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "file is too large to be compressed",
        ));
    }

    // TODO: Try a local buffer for non-alloc fast path
    let path = CString::new(path.as_os_str().as_bytes())?;

    let mut statfs_buf = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: path is a valid pointer, and null terminated, statfs_buf is a valid ptr, and is used as an out ptr
    let rc = unsafe { libc::statfs(path.as_ptr(), statfs_buf.as_mut_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: if statfs returned non-zero, we returned already, it should have filled in statfs_buf
    let statfs_buf = unsafe { statfs_buf.assume_init_ref() };

    // TODO: let is_apfs = statfs_buf.f_fstypename.starts_with(APFS_CHARS);
    let is_zfs = statfs_buf.f_fssubtype == ZFS_SUBTYPE;

    if is_zfs {
        // ZFS doesn't do HFS/decmpfs compression. It may pretend to, but in
        // that case it will *de*compress the data before committing it. We
        // won't play that game, wasting cycles and rewriting data for nothing.
        return Err(io::Error::new(io::ErrorKind::Other, "filesystem is zfs"));
    }

    if xattr::is_present(&path, resource_fork::XATTR_NAME)?
        || xattr::is_present(&path, decmpfs::XATTR_NAME)?
    {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "file already has required xattrs",
        ));
    }

    let root_path = cstr_from_bytes_until_null(&statfs_buf.f_mntonname)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "mount name invalid"))?;
    if !vol_supports_compression_cap(root_path)? {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "compression unsupported by fs",
        ));
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

struct ForceWritableFile {
    file: File,
    permissions: Option<Permissions>,
}

impl ForceWritableFile {
    fn open(path: &Path, metadata: &Metadata) -> io::Result<Self> {
        let old_perm = metadata.permissions();
        let new_perm = Permissions::from_mode(
            old_perm.mode() | u32::from(libc::S_IWUSR) | u32::from(libc::S_IRUSR),
        );
        let reset_permissions = if old_perm == new_perm {
            None
        } else {
            fs::set_permissions(path, new_perm)?;
            Some(old_perm)
        };

        let file = match File::options().read(true).write(true).open(path) {
            Ok(file) => file,
            Err(e) => {
                if let Some(permissions) = reset_permissions {
                    let _res = fs::set_permissions(path, permissions);
                }
                return Err(e);
            }
        };
        Ok(Self {
            file,
            permissions: reset_permissions,
        })
    }
}

impl Deref for ForceWritableFile {
    type Target = File;

    fn deref(&self) -> &File {
        &self.file
    }
}

impl Drop for ForceWritableFile {
    fn drop(&mut self) {
        if let Some(permissions) = self.permissions.clone() {
            let res = self.file.set_permissions(permissions);
            if let Err(e) = res {
                tracing::error!("unable to reset permissions: {}", e);
            }
        }
    }
}

pub struct FileCompressor {
    bg_threads: BackgroundThreads,
}

impl FileCompressor {
    #[must_use]
    pub fn new(compressor_kind: compressor::Kind) -> Self {
        Self {
            bg_threads: BackgroundThreads::new(compressor_kind),
        }
    }

    #[tracing::instrument(skip_all, fields(path = %path.display()))]
    pub fn compress_path(
        &mut self,
        path: PathBuf,
        progress: impl Progress + Send + Sync + 'static,
    ) {
        let progress: Box<dyn Progress + Send + Sync> = Box::new(progress);
        self.bg_threads.submit(path, progress);
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

#[must_use]
const fn checked_add_signed(x: u64, i: i64) -> Option<u64> {
    if i >= 0 {
        x.checked_add(i as u64)
    } else {
        x.checked_sub(i.unsigned_abs())
    }
}

#[must_use]
const fn round_to_block_size(size: u64, block_size: u64) -> u64 {
    match size % block_size {
        0 => size,
        r => size + (block_size - r),
    }
}
