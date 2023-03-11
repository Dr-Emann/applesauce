use crate::progress::{Progress, SkipReason};
use crate::threads::Mode;
use ignore::WalkState;
use std::fs::Metadata;
use std::path::{Path, PathBuf};

pub struct Walker<'a, P> {
    paths: ignore::WalkParallel,
    progress: &'a P,
}

impl<'a, P: Progress + Send + Sync> Walker<'a, P> {
    pub fn new<'b>(paths: impl IntoIterator<Item = &'b Path>, progress: &'a P) -> Self {
        let mut paths = paths.into_iter();
        let first = paths.next().expect("No paths given");
        let mut builder = ignore::WalkBuilder::new(first);
        // We don't want to ignore hidden, from gitignore, etc
        builder.standard_filters(false);
        // Add the rest of the paths
        paths.for_each(|p| {
            builder.add(p);
        });

        Self {
            paths: builder.build_parallel(),
            progress,
        }
    }

    pub fn run(self, mode: Mode, f: impl Fn(PathBuf, Metadata) + Send + Sync) {
        self.paths.run(|| {
            Box::new(|entry| {
                handle_entry(entry, mode, self.progress, &f);
                WalkState::Continue
            })
        })
    }
}

fn handle_entry<F>(
    entry: Result<ignore::DirEntry, ignore::Error>,
    mode: Mode,
    progress: &impl Progress,
    f: &F,
) where
    F: Fn(PathBuf, Metadata),
{
    let entry = match entry {
        Ok(entry) => entry,
        Err(e) => {
            progress.error(Path::new("?"), &format!("error scanning: {}", e));
            return;
        }
    };
    let path = entry.path();

    let file_type = entry
        .file_type()
        .expect("Only stdin should have no file_type");

    if file_type.is_dir() {
        return;
    }

    let metadata = match entry.metadata() {
        Ok(metadata) => metadata,
        Err(e) => {
            progress.file_skipped(path, SkipReason::ReadError(e.into_io_error().unwrap()));
            return;
        }
    };
    let res = if mode.is_compressing() {
        crate::check_compressible(path, &metadata)
    } else {
        crate::check_decompressible(&metadata)
    };
    if let Err(e) = res {
        progress.file_skipped(path, e);
        return;
    }

    f(entry.into_path(), metadata);
}
