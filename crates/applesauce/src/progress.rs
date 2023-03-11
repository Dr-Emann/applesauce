use std::path::Path;
use std::{fmt, io};

#[derive(Debug)]
pub enum SkipReason {
    NotFile,
    AlreadyCompressed,
    NotCompressed,
    TooLarge(u64),
    ReadError(io::Error),
    ZfsFilesystem,
    HasRequiredXattr,
    FsNotSupported,
}

pub trait Progress {
    type Task: Task;

    fn error(&self, path: &Path, message: &str);
    fn file_skipped(&self, _path: &Path, _why: SkipReason) {}
    fn file_task(&self, path: &Path, size: u64) -> Self::Task;
}

pub trait Task {
    fn increment(&self, amt: u64);
    fn error(&self, message: &str);
    fn not_compressible_enough(&self, _path: &Path) {}
}

impl<P: Progress> Progress for &'_ P {
    type Task = P::Task;

    fn error(&self, path: &Path, message: &str) {
        P::error(self, path, message)
    }

    fn file_skipped(&self, path: &Path, why: SkipReason) {
        P::file_skipped(self, path, why)
    }

    fn file_task(&self, path: &Path, size: u64) -> Self::Task {
        P::file_task(self, path, size)
    }
}

impl<T: Task> Task for &'_ T {
    fn increment(&self, amt: u64) {
        T::increment(self, amt)
    }

    fn error(&self, message: &str) {
        T::error(self, message)
    }

    fn not_compressible_enough(&self, path: &Path) {
        T::not_compressible_enough(self, path)
    }
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            SkipReason::NotFile => write!(f, "Not a file"),
            SkipReason::AlreadyCompressed => write!(f, "Already compressed"),
            SkipReason::NotCompressed => write!(f, "Not compressed"),
            SkipReason::TooLarge(size) => write!(f, "File too large: {size} > {}", u32::MAX),
            SkipReason::ReadError(ref err) => write!(f, "Read error: {err}"),
            SkipReason::ZfsFilesystem => write!(f, "ZFS filesystem (not supported)"),
            SkipReason::HasRequiredXattr => write!(f, "Compression xattrs already present"),
            SkipReason::FsNotSupported => write!(f, "Filesystem does not support compression"),
        }
    }
}

impl From<io::Error> for SkipReason {
    fn from(err: io::Error) -> SkipReason {
        SkipReason::ReadError(err)
    }
}
