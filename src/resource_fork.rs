use crate::checked_add_signed;
use libc::XATTR_SHOWCOMPRESSION;
use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::io::AsRawFd;
use std::{cmp, io, ptr};

pub const XATTR_NAME: &CStr = crate::cstr!("com.apple.ResourceFork");

pub struct ResourceFork<'a> {
    file: &'a File,
    offset: u32,
}

impl<'a> ResourceFork<'a> {
    #[must_use]
    pub fn new(file: &'a File) -> Self {
        Self { file, offset: 0 }
    }
}

impl<'a> io::Write for ResourceFork<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len: u32 = buf
            .len()
            .try_into()
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        let end_offset = self.offset.checked_add(len).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Other,
                "unable to fit resource fork in 32 bits",
            )
        })?;
        // SAFETY:
        // fd is valid
        // xattr name is valid
        let rc = unsafe {
            libc::fsetxattr(
                self.file.as_raw_fd(),
                XATTR_NAME.as_ptr(),
                buf.as_ptr().cast(),
                buf.len(),
                self.offset,
                XATTR_SHOWCOMPRESSION,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        self.offset = end_offset;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for ResourceFork<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Despite the manpage for getxattr saying:
        // > On success, the size of the extended attribute data is returned
        // it actually returns the size remaining _after_ the passed index
        let rc = unsafe {
            libc::fgetxattr(
                self.file.as_raw_fd(),
                XATTR_NAME.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                self.offset,
                XATTR_SHOWCOMPRESSION,
            )
        };
        let remaining_len = if rc < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ENOATTR) {
                0
            } else {
                return Err(e);
            }
        } else {
            rc as usize
        };
        Ok(cmp::min(remaining_len, buf.len()))
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let rc = unsafe {
            libc::fgetxattr(
                self.file.as_raw_fd(),
                XATTR_NAME.as_ptr(),
                ptr::null_mut(),
                0,
                0,
                XATTR_SHOWCOMPRESSION,
            )
        };
        let xattr_len = if rc < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ENOATTR) {
                0
            } else {
                return Err(e);
            }
        } else {
            rc as usize
        };

        let remaining_bytes = xattr_len.saturating_sub(self.offset.try_into().unwrap());
        let buf_start = buf.len();
        buf.resize(buf_start + remaining_bytes, 0);

        let result = self.read(&mut buf[buf_start..]);
        match result {
            Ok(n) => {
                if n < remaining_bytes {
                    buf.truncate(buf_start + n);
                }
            }
            Err(_) => {
                buf.truncate(buf_start);
            }
        }
        result
    }
}

impl Seek for ResourceFork<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_offset: u32 = match pos {
            SeekFrom::Start(i) => i.try_into().map_err(|_| io::ErrorKind::InvalidInput)?,
            SeekFrom::End(i) => {
                // SAFETY:
                // fd is valid
                // xattr name is valid, and null terminated
                // value == NULL && size == 0 is allowed, to just return the length of the value
                let mut rc = unsafe {
                    libc::fgetxattr(
                        self.file.as_raw_fd(),
                        XATTR_NAME.as_ptr(),
                        ptr::null_mut(),
                        0,
                        0,
                        XATTR_SHOWCOMPRESSION,
                    )
                };
                if rc < 0 {
                    let e = io::Error::last_os_error();
                    if e.raw_os_error() == Some(libc::ENOATTR) {
                        rc = 0;
                    } else {
                        return Err(e);
                    }
                }
                let end: u64 = rc.try_into().unwrap();
                let offset = checked_add_signed(end, i).ok_or(io::ErrorKind::InvalidInput)?;
                offset.try_into().map_err(|_| io::ErrorKind::InvalidInput)?
            }
            SeekFrom::Current(i) => {
                let offset =
                    checked_add_signed(self.offset.into(), i).ok_or(io::ErrorKind::InvalidInput)?;
                offset.try_into().map_err(|_| io::ErrorKind::InvalidInput)?
            }
        };
        self.offset = new_offset;
        Ok(new_offset.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::has_xattr;
    use std::ffi::CString;
    use std::fs;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
    use tempfile::NamedTempFile;

    #[test]
    fn no_create_without_write() {
        let file = NamedTempFile::new().unwrap();
        let mut rfork = ResourceFork::new(file.as_file());
        let path = CString::new(file.path().as_os_str().as_bytes()).unwrap();
        assert!(!has_xattr(&path, XATTR_NAME).unwrap());
        assert_eq!(rfork.seek(SeekFrom::Start(10)).unwrap(), 10);
        assert!(!has_xattr(&path, XATTR_NAME).unwrap());
        assert_eq!(rfork.seek(SeekFrom::Current(1)).unwrap(), 11);
        assert!(!has_xattr(&path, XATTR_NAME).unwrap());
        assert_eq!(rfork.seek(SeekFrom::End(0)).unwrap(), 0);
        assert!(!has_xattr(&path, XATTR_NAME).unwrap());
    }

    #[test]
    fn create_by_write() {
        let file = NamedTempFile::new().unwrap();
        let mut rfork = ResourceFork::new(file.as_file());
        let path = CString::new(file.path().as_os_str().as_bytes()).unwrap();

        let data = b"hi there";
        assert_eq!(rfork.write(data).unwrap(), data.len());
        rfork.flush().unwrap();
        assert!(has_xattr(&path, XATTR_NAME).unwrap());
        let content = fs::read(file.path().join("..namedfork/rsrc")).unwrap();
        assert_eq!(content, data);
    }

    #[test]
    fn read_not_exist() {
        let file = tempfile::tempfile().unwrap();
        let mut rfork = ResourceFork::new(&file);

        let mut buf = [0; 1024];
        let mut buf_vec = Vec::new();
        assert_eq!(rfork.read(&mut buf).unwrap(), 0);
        assert_eq!(rfork.read_to_end(&mut buf_vec).unwrap(), 0);
        assert!(buf_vec.is_empty());

        assert_eq!(rfork.seek(SeekFrom::Start(10)).unwrap(), 10);
        assert_eq!(rfork.read(&mut buf).unwrap(), 0);
        assert_eq!(rfork.read_to_end(&mut buf_vec).unwrap(), 0);
        assert!(buf_vec.is_empty());
    }

    #[test]
    fn read_past_end() {
        let file = tempfile::tempfile().unwrap();
        let mut rfork = ResourceFork::new(&file);

        let data = b"hi there";
        assert_eq!(rfork.write(data).unwrap(), data.len());

        let mut buf = [0; 1024];
        let mut buf_vec = vec![1, 2, 3];
        // at end already, should empty read
        assert_eq!(rfork.read(&mut buf).unwrap(), 0);
        assert_eq!(rfork.offset as usize, data.len());

        assert_eq!(
            rfork.seek(SeekFrom::Current(10)).unwrap(),
            data.len() as u64 + 10
        );
        assert_eq!(rfork.read(&mut buf).unwrap(), 0);
        assert_eq!(rfork.read_to_end(&mut buf_vec).unwrap(), 0);
        assert_eq!(buf_vec, [1, 2, 3]);
    }

    #[test]
    fn read() {
        let file = tempfile::tempfile().unwrap();
        let mut rfork = ResourceFork::new(&file);

        let data = b"hi there";
        assert_eq!(rfork.write(data).unwrap(), data.len());

        assert_eq!(
            rfork.seek(SeekFrom::Current(-1)).unwrap(),
            data.len() as u64 - 1
        );

        let mut buf = [0; 1024];
        let mut buf_vec = vec![1, 2, 3];
        assert_eq!(rfork.read_to_end(&mut buf_vec).unwrap(), 1);
        assert_eq!(buf_vec, [1, 2, 3, b'e']);

        rfork.rewind().unwrap();
        assert_eq!(rfork.read(&mut buf).unwrap(), data.len());
        assert_eq!(&buf[..data.len()], data);
    }
}
