use crate::times;
use std::ffi::{c_void, CStr, CString};
use std::fs::File;
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::{io, mem, ptr};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Saved {
    create_time: libc::timespec,
    mod_time: libc::timespec,
    access_time: libc::timespec,
    add_time: libc::timespec,
}

impl Saved {
    fn from_attr_buf(attr_buf: &AttrGetBuf) -> Self {
        assert_eq!(attr_buf.len as usize, mem::size_of_val(attr_buf));
        Self {
            create_time: attr_buf.create_time,
            mod_time: attr_buf.mod_time,
            access_time: attr_buf.access_time,
            add_time: attr_buf.add_time,
        }
    }
}

#[repr(C, packed(4))]
struct AttrGetBuf {
    len: u32,
    returned_attrs: libc::attribute_set_t,
    create_time: libc::timespec,
    mod_time: libc::timespec,
    access_time: libc::timespec,
    add_time: libc::timespec,
}

#[repr(C, packed(4))]
struct AttrSetBuf {
    create_time: libc::timespec,
    mod_time: libc::timespec,
    access_time: libc::timespec,
    add_time: libc::timespec,
}

trait GetSet {
    fn get_times(&self) -> io::Result<Saved>;

    fn reset_times(&self, saved: &Saved) -> io::Result<()>;
}

fn attrlist_get() -> libc::attrlist {
    // SAFETY: libc::attrlist is a POD c struct, zero is a valid value for all fields.
    let mut attrlist: libc::attrlist = unsafe { mem::zeroed() };
    attrlist.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    attrlist.commonattr = libc::ATTR_CMN_RETURNED_ATTRS
        | libc::ATTR_CMN_CRTIME
        | libc::ATTR_CMN_MODTIME
        | libc::ATTR_CMN_ACCTIME
        | libc::ATTR_CMN_ADDEDTIME;
    attrlist
}

fn attrlist_set() -> libc::attrlist {
    // SAFETY: libc::attrlist is a POD c struct, zero is a valid value for all fields.
    let mut attrlist: libc::attrlist = unsafe { mem::zeroed() };
    attrlist.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    attrlist.commonattr = libc::ATTR_CMN_CRTIME
        | libc::ATTR_CMN_MODTIME
        | libc::ATTR_CMN_ACCTIME
        | libc::ATTR_CMN_ADDEDTIME;
    attrlist
}

impl GetSet for File {
    fn get_times(&self) -> io::Result<Saved> {
        let mut attrlist = attrlist_get();

        let mut attr_buf: MaybeUninit<AttrGetBuf> = MaybeUninit::uninit();

        // SAFETY: attr_buf is filled by a successful call
        unsafe {
            let rc = libc::fgetattrlist(
                self.as_raw_fd(),
                ptr::addr_of_mut!(attrlist).cast::<c_void>(),
                attr_buf.as_mut_ptr().cast::<c_void>(),
                mem::size_of::<AttrGetBuf>(),
                libc::FSOPT_PACK_INVAL_ATTRS,
            );
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
            let attr_buf = attr_buf.assume_init_ref();
            Ok(Saved::from_attr_buf(attr_buf))
        }
    }

    fn reset_times(&self, saved: &Saved) -> io::Result<()> {
        let mut attrlist = attrlist_set();

        let mut attr_buf = AttrSetBuf {
            create_time: saved.create_time,
            mod_time: saved.mod_time,
            access_time: saved.access_time,
            add_time: saved.add_time,
        };

        // Safety: attr_buf is filled by a successful call, the fd is valid
        unsafe {
            let rc = libc::fsetattrlist(
                self.as_raw_fd(),
                ptr::addr_of_mut!(attrlist).cast::<c_void>(),
                ptr::addr_of_mut!(attr_buf).cast::<c_void>(),
                mem::size_of::<AttrSetBuf>(),
                0,
            );
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }

            Ok(())
        }
    }
}

impl GetSet for CStr {
    fn get_times(&self) -> io::Result<Saved> {
        let mut attrlist = attrlist_get();

        let mut attr_buf: MaybeUninit<AttrGetBuf> = MaybeUninit::uninit();

        // Safety: attr_buf is filled by a successful call
        unsafe {
            let rc = libc::getattrlist(
                self.as_ptr(),
                ptr::addr_of_mut!(attrlist).cast::<c_void>(),
                attr_buf.as_mut_ptr().cast::<c_void>(),
                mem::size_of::<AttrGetBuf>(),
                libc::FSOPT_PACK_INVAL_ATTRS,
            );
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
            let attr_buf = attr_buf.assume_init_ref();
            Ok(Saved::from_attr_buf(attr_buf))
        }
    }

    fn reset_times(&self, saved: &Saved) -> io::Result<()> {
        let mut attrlist = attrlist_set();

        let mut attr_buf = AttrSetBuf {
            create_time: saved.create_time,
            mod_time: saved.mod_time,
            access_time: saved.access_time,
            add_time: saved.add_time,
        };

        // Safety: attr_buf is filled by a successful call
        unsafe {
            let rc = libc::setattrlist(
                self.as_ptr(),
                ptr::addr_of_mut!(attrlist).cast::<c_void>(),
                ptr::addr_of_mut!(attr_buf).cast::<c_void>(),
                mem::size_of::<AttrSetBuf>(),
                0,
            );
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }

            Ok(())
        }
    }
}

impl GetSet for Path {
    fn get_times(&self) -> io::Result<Saved> {
        let cstr = CString::new(self.as_os_str().as_bytes())?;
        <CStr as GetSet>::get_times(&cstr)
    }

    fn reset_times(&self, saved: &Saved) -> io::Result<()> {
        let cstr = CString::new(self.as_os_str().as_bytes())?;
        <CStr as GetSet>::reset_times(&cstr, saved)
    }
}

#[tracing::instrument(level = "debug")]
#[inline]
pub fn save_times<F: GetSet + std::fmt::Debug + ?Sized>(f: &F) -> io::Result<Saved> {
    f.get_times()
}

#[tracing::instrument(level = "debug")]
#[inline]
pub fn reset_times<F: GetSet + std::fmt::Debug + ?Sized>(f: &F, saved: &Saved) -> io::Result<()> {
    f.reset_times(saved)
}

#[derive(Debug)]
pub struct Resetter {
    dir_path: CString,
    saved_times: Saved,
}

impl Resetter {
    pub fn new(path: &Path, saved_times: Saved) -> io::Result<Self> {
        let dir_path = CString::new(path.as_os_str().as_bytes())?;
        Ok(Self {
            dir_path,
            saved_times,
        })
    }
}

impl Drop for Resetter {
    fn drop(&mut self) {
        let _ = times::reset_times(self.dir_path.as_c_str(), &self.saved_times);
    }
}
