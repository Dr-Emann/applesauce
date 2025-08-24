use crate::progress::Progress;
use crate::threads::OperationContext;
use crate::times;
use std::fs::FileType;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn walk_dir_over(
    path: &Path,
    context: Arc<OperationContext>,
) -> jwalk::WalkDirGeneric<((), State)> {
    let walker = jwalk::WalkDirGeneric::new(path);
    walker.process_read_dir(
        move |_depth,
              path: &Path,
              _state,
              entries: &mut Vec<jwalk::Result<jwalk::DirEntry<((), State)>>>| {
            let mut reset_times: Option<State> = None;
            // Remove ignored directories from the list of entries.
            // Also, add the client state to the entry.
            entries.retain_mut(|entry| {
                if let Ok(entry) = entry {
                    if entry.file_type().is_dir() && context.is_temp_dir(&entry.path()) {
                        return false;
                    }
                    #[allow(clippy::filetype_is_file)]
                    if entry.file_type().is_file() {
                        let reset_times = match &mut reset_times {
                            Some(reset_times) => reset_times,
                            None => reset_times.insert(
                                times::save_times(path)
                                    .and_then(|saved_times| times::Resetter::new(path, saved_times))
                                    .ok()
                                    .map(Arc::new),
                            ),
                        };
                        entry.client_state.clone_from(reset_times);
                    }
                }
                true
            });
        },
    )
}

type State = Option<Arc<times::Resetter>>;

pub struct Walker<'a, P> {
    paths: Vec<&'a Path>,
    progress: &'a P,
}

impl<'a, P: Progress + Send + Sync> Walker<'a, P> {
    pub fn new(progress: &'a P) -> Self {
        Self {
            paths: Vec::new(),
            progress,
        }
    }

    pub fn add_path(&mut self, path: &'a Path) {
        self.paths.push(path);
    }

    pub fn run(
        self,
        context: &Arc<OperationContext>,
        f: impl Fn(FileType, PathBuf, Option<Arc<times::Resetter>>) + Send + Sync,
    ) {
        for path in self.paths {
            let walker = walk_dir_over(path, Arc::clone(context));
            for entry in walker {
                let mut entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        self.progress
                            .error(Path::new("?"), &format!("error scanning: {e}"));
                        continue;
                    }
                };
                let path = entry.path();
                let metadata = match entry.metadata() {
                    Ok(metadata) => metadata,
                    Err(e) => {
                        self.progress
                            .error(&path, &format!("error getting metadata: {e}"));
                        continue;
                    }
                };
                if metadata.is_dir() {
                    continue;
                }
                f(metadata.file_type(), path, entry.client_state.take())
            }
        }
    }
}
