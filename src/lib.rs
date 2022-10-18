pub mod compressor;
mod decmpfs;
pub mod resource_fork;

use crate::decmpfs::{CompressionType, Storage};
use crate::resource_fork::ResourceFork;
use libc::c_char;
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata, Permissions};
use std::io::prelude::*;
use std::io::{BufWriter, SeekFrom};
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::{fs, io, mem, ptr};

macro_rules! cstr {
    ($s:literal) => {{
        // TODO: Check for nulls
        // SAFETY: definitely null terminated, at worst terminated early
        unsafe { CStr::from_bytes_with_nul_unchecked(concat!($s, "\0").as_bytes()) }
    }};
}

use crate::compressor::{Compressor, CompressorImpl};
pub(crate) use cstr;

const BLOCK_SIZE: usize = 0x10000;
const MAX_COMPRESSION_SIZE: u64 = (1 << 31) - 1;

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

fn num_blocks(size: u64) -> u64 {
    (size + (BLOCK_SIZE as u64 - 1)) / (BLOCK_SIZE as u64)
}

pub fn check_compressable(path: &Path, metadata: &Metadata) -> io::Result<()> {
    if !metadata.is_file() {
        return Err(io::Error::new(io::ErrorKind::Other, "not a file"));
    }
    if metadata.st_flags() & libc::UF_COMPRESSED != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "file already compressed",
        ));
    }

    let blocks = num_blocks(metadata.len());
    // TODO: why 0x13A, why * 9?
    if metadata.len() + 0x13A + (blocks * 9) > MAX_COMPRESSION_SIZE {
        return Err(io::Error::new(io::ErrorKind::Other, "file too large"));
    }

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

    if has_xattr(&path, resource_fork::XATTR_NAME)? || has_xattr(&path, decmpfs::XATTR_NAME)? {
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
    if vol_attrs.length != mem::size_of::<VolAttrs>() as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "getattrlist returned bad size",
        ));
    }

    const IDX: usize = libc::VOL_CAPABILITIES_FORMAT;
    const MASK: libc::attrgroup_t = libc::VOL_CAP_FMT_DECMPFS_COMPRESSION;
    Ok(vol_attrs.vol_attrs.valid[IDX] & vol_attrs.vol_attrs.capabilities[IDX] & MASK != 0)
}

fn has_xattr(path: &CStr, xattr_name: &CStr) -> io::Result<bool> {
    // SAFETY:
    // path/xattr_name are valid pointers and are null terminated
    // value == NULL, size === 0 is allowed to just return the size
    let rc = unsafe {
        libc::getxattr(
            path.as_ptr(),
            xattr_name.as_ptr(),
            ptr::null_mut(),
            0,
            0,
            libc::XATTR_SHOWCOMPRESSION,
        )
    };
    if rc == -1 {
        let last_error = io::Error::last_os_error();
        return if last_error.raw_os_error() == Some(libc::ENOATTR) {
            Ok(false)
        } else {
            Err(last_error)
        };
    }
    Ok(true)
}

#[derive(Debug, Copy, Clone)]
pub enum Compression {
    ZLIB,
    LZVN,
    LZFSE,
}

impl Compression {
    #[must_use]
    pub fn supported(self) -> bool {
        match self {
            Compression::ZLIB => {
                cfg!(feature = "zlib")
            }
            Compression::LZVN | Compression::LZFSE => {
                // TODO:
                false
            }
        }
    }
}

struct CompressionConfig {
    max_size: u64,
    allow_large_blocks: bool,
    compression: Compression,
}

struct ForceWritableFile {
    file: File,
    permissions: Option<Permissions>,
}

impl ForceWritableFile {
    fn new(path: &Path, metadata: &Metadata) -> io::Result<Self> {
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
                    let _ = fs::set_permissions(path, permissions);
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
        if let Some(permissions) = self.permissions.take() {
            let res = self.file.set_permissions(permissions);
            if let Err(e) = res {
                eprintln!("Error resetting permissions {}", e);
            }
        }
    }
}

pub fn compress(path: &Path, metadata: &Metadata, comp: &mut Compressor) -> io::Result<()> {
    check_compressable(path, metadata)?;

    let file = ForceWritableFile::new(path, metadata)?;
    let mut compressed_data = BufWriter::new(ResourceFork::new(&file));

    let metadata = file.metadata()?;
    let file_size = metadata.len();
    let block_count: u32 = num_blocks(file_size)
        .try_into()
        .map_err(|_| io::ErrorKind::InvalidInput)?;

    compressed_data.seek(SeekFrom::Start(comp.blocks_start(block_count.into())))?;

    let mut block_sizes = Vec::with_capacity(block_count as usize);

    raw_compress_into(&*file, &mut compressed_data, comp, &mut block_sizes)?;

    comp.finish(&mut compressed_data, &block_sizes)?;
    compressed_data.flush()?;
    drop(compressed_data);

    let mut decomp_xattr_val = Vec::with_capacity(decmpfs::DiskHeader::SIZE);
    let header = decmpfs::DiskHeader {
        compression_type: CompressionType {
            compression: Compression::ZLIB,
            storage: Storage::ResourceFork,
        }
        .raw_type(),
        uncompressed_size: file_size,
    };
    header.write_into(&mut decomp_xattr_val)?;
    // SAFETY:
    // fd is valid
    // xattr name is valid and null terminated
    // value is valid, writable, and initialized up to `.len()` bytes
    let rc = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            decmpfs::XATTR_NAME.as_ptr(),
            decomp_xattr_val.as_ptr().cast(),
            decomp_xattr_val.len(),
            0,
            libc::XATTR_SHOWCOMPRESSION,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    file.set_len(0)?;

    // SAFETY: fd is valid
    let rc = unsafe { libc::fchflags(file.as_raw_fd(), metadata.st_flags() | libc::UF_COMPRESSED) };
    if rc < 0 {
        let e = io::Error::last_os_error();
        // TODO: Roll back better
        return Err(e);
    }

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
    unsafe {
        libc::futimens(file.as_raw_fd(), times.as_ptr());
    }

    Ok(())
}

fn try_read_all<R: Read>(mut r: R, buf: &mut [u8]) -> io::Result<usize> {
    let full_len = buf.len();
    let mut remaining = buf;
    loop {
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
    Ok(full_len - remaining.len())
}

fn raw_compress_into<R: Read, W: Write + Seek>(
    mut r: R,
    mut w: W,
    comp: &mut Compressor,
    block_sizes: &mut Vec<u32>,
) -> io::Result<()> {
    let mut read_buffer = vec![0; BLOCK_SIZE];
    let mut write_buffer = vec![0; BLOCK_SIZE + 1024];

    loop {
        let n = try_read_all(&mut r, &mut read_buffer)?;
        if n == 0 {
            break;
        }

        let dst_len = comp.compress(&mut write_buffer, &read_buffer[..n])?;
        w.write_all(&write_buffer[..dst_len])?;
        block_sizes.push(dst_len.try_into().unwrap());
    }
    Ok(())
}

fn checked_add_signed(x: u64, i: i64) -> Option<u64> {
    if i >= 0 {
        x.checked_add(i as u64)
    } else {
        x.checked_sub(i.unsigned_abs())
    }
}
