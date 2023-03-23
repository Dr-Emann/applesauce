use crate::progress::Progress;
use ignore::WalkState;
use std::fs::FileType;
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

    pub fn run(self, f: impl Fn(FileType, PathBuf) + Send + Sync) {
        self.paths.run(|| {
            Box::new(|entry| {
                handle_entry(entry, self.progress, &f);
                WalkState::Continue
            })
        })
    }
}

fn handle_entry(
    entry: Result<ignore::DirEntry, ignore::Error>,
    progress: &impl Progress,
    f: &impl Fn(FileType, PathBuf),
) {
    let entry = match entry {
        Ok(entry) => entry,
        Err(e) => {
            progress.error(Path::new("?"), &format!("error scanning: {}", e));
            return;
        }
    };
    let file_type = entry
        .file_type()
        .expect("Only stdin should have no file_type");

    if file_type.is_dir() {
        return;
    }

    f(file_type, entry.into_path());
}
