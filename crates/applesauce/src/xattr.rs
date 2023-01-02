use memchr::memchr;
use std::cmp::Ordering;
use std::ffi::CStr;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::{io, ptr};

#[allow(dead_code)]
pub fn remove(file: &File, xattr_name: &CStr) -> io::Result<()> {
    // SAFETY: fd is valid, xattr_name is valid, and null terminated
    let rc = unsafe {
        libc::fremovexattr(
            file.as_raw_fd(),
            xattr_name.as_ptr(),
            libc::XATTR_SHOWCOMPRESSION,
        )
    };
    if rc == -1 {
        let last_error = io::Error::last_os_error();
        if last_error.raw_os_error() != Some(libc::ENOATTR) {
            return Err(last_error);
        };
    }
    Ok(())
}

pub fn len(path: &CStr, xattr_name: &CStr) -> io::Result<Option<usize>> {
    // SAFETY:
    // path/xattr_name are valid pointers and are null terminated
    // value == NULL, size == 0 is allowed to just return the size
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
            Ok(None)
        } else {
            Err(last_error)
        };
    }
    Ok(Some(rc as usize))
}

pub fn is_present(path: &CStr, xattr_name: &CStr) -> io::Result<bool> {
    len(path, xattr_name).map(|len| len.is_some())
}

pub fn set(file: &File, xattr_name: &CStr, data: &[u8], offset: u32) -> io::Result<()> {
    // SAFETY:
    // fd is valid
    // xattr name is valid and null terminated
    // value is valid, writable, and initialized up to `.len()` bytes
    let rc = unsafe {
        libc::fsetxattr(
            file.as_raw_fd(),
            xattr_name.as_ptr(),
            data.as_ptr().cast(),
            data.len(),
            offset,
            libc::XATTR_SHOWCOMPRESSION,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn read(path: &CStr, xattr_name: &CStr) -> io::Result<Option<Vec<u8>>> {
    let len = match len(path, xattr_name)? {
        Some(len) => len,
        None => return Ok(None),
    };

    let mut buf = vec![0; len];

    loop {
        // SAFETY:
        // path/xattr_name are valid pointers and are null terminated
        // value == NULL, size == 0 is allowed to just return the size
        let rc = unsafe {
            libc::getxattr(
                path.as_ptr(),
                xattr_name.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                0,
                libc::XATTR_SHOWCOMPRESSION,
            )
        };
        if rc < 0 {
            let last_error = io::Error::last_os_error();
            return if last_error.raw_os_error() == Some(libc::ENOATTR) {
                Ok(None)
            } else {
                Err(last_error)
            };
        }
        let new_len = rc as usize;
        match len.cmp(&new_len) {
            Ordering::Less => {
                buf.truncate(new_len);
                break;
            }
            Ordering::Equal => break,
            Ordering::Greater => {
                buf.resize(new_len, 0);
            }
        }
    }
    Ok(Some(buf))
}

fn raw_names(path: &CStr) -> io::Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        // Safety:
        // path is valid, and null terminated
        // it is safe to pass list=null,size=0
        let rc = unsafe {
            libc::listxattr(
                path.as_ptr(),
                ptr::null_mut(),
                0,
                libc::XATTR_SHOWCOMPRESSION,
            )
        };
        if rc < 0 {
            let e = io::Error::last_os_error();
            return match e.raw_os_error() {
                Some(libc::ENOTSUP | libc::EPERM) => Ok(Vec::new()),
                _ => Err(e),
            };
        }
        let size = rc as usize;
        buf.resize(size, 0);

        // Safety:
        // path is valid, and null terminated
        // buf is valid, and writable for len bytes
        let rc = unsafe {
            libc::listxattr(
                path.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                libc::XATTR_SHOWCOMPRESSION,
            )
        };
        if rc < 0 {
            let e = io::Error::last_os_error();
            return match e.raw_os_error() {
                Some(libc::ERANGE) => continue,
                _ => Err(e),
            };
        }
        let size = rc as usize;
        buf.truncate(size);
        break;
    }
    Ok(buf)
}

pub fn with_names<F: FnMut(&CStr) -> io::Result<()>>(path: &CStr, mut f: F) -> io::Result<()> {
    let raw_buf = raw_names(path)?;
    let mut raw_buf = &raw_buf[..];

    while !raw_buf.is_empty() {
        let next_end = memchr(0, raw_buf).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Other,
                "expected null terminator in xattr names",
            )
        })?;
        let end_including_term = next_end + 1;
        let name = CStr::from_bytes_with_nul(&raw_buf[..end_including_term])
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        f(name)?;
        raw_buf = &raw_buf[end_including_term..];
    }

    Ok(())
}
