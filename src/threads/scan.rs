use ignore::WalkState;
use std::fs::Metadata;
use std::path::{Path, PathBuf};

pub fn for_each_recursive<'a, F>(paths: impl IntoIterator<Item = &'a Path>, f: F)
where
    F: Fn(PathBuf, Metadata) + Send + Sync,
{
    let mut paths = paths.into_iter();
    let Some(first) = paths.next() else { return };
    let mut builder = ignore::WalkBuilder::new(first);
    // We don't want to ignore hidden, from gitignore, etc
    builder.standard_filters(false);
    // Add the rest of the paths
    paths.for_each(|p| {
        builder.add(p);
    });

    let walker = builder.build_parallel();
    walker.run(|| {
        Box::new(|entry| {
            handle_entry(entry, &f);
            WalkState::Continue
        })
    })
}

fn handle_entry<F>(entry: Result<ignore::DirEntry, ignore::Error>, f: &F)
where
    F: Fn(PathBuf, Metadata),
{
    let entry = match entry {
        Ok(entry) => entry,
        Err(e) => {
            tracing::warn!("error scanning: {e}");
            return;
        }
    };
    let path = entry.path();
    let metadata = match entry.metadata() {
        Ok(metadata) => metadata,
        Err(e) => {
            tracing::warn!("unable to get metadata for {}: {e}", path.display());
            return;
        }
    };
    if let Err(e) = crate::check_compressible(path, &metadata) {
        tracing::debug!("{} is not compressible: {e}", path.display());
        return;
    }

    f(entry.into_path(), metadata);
}
