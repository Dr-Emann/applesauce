use crate::checked_add_signed;
use libc::XATTR_SHOWCOMPRESSION;
use std::ffi::CStr;
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::os::unix::io::AsRawFd;
use std::{io, ptr};

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
        self.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let len: u32 = buf
            .len()
            .try_into()
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        let end_offset = self
            .offset
            .checked_add(len)
            .ok_or(io::ErrorKind::UnexpectedEof)?;
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
        Ok(())
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
                if rc < 0 {
                    return Err(io::Error::last_os_error());
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
