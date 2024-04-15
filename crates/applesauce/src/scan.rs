use crate::progress::Progress;
use crate::tmpdir_paths::TmpdirPaths;
use std::collections::HashSet;
use std::ffi::CString;
use std::fs::FileType;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
pub struct ResetTimes {
    dir_path: CString,
    metadata: std::fs::Metadata,
}

impl ResetTimes {
    fn new(path: &Path, metadata: std::fs::Metadata) -> Self {
        Self {
            dir_path: CString::new(path.as_os_str().as_bytes()).unwrap(),
            metadata,
        }
    }
}

impl Drop for ResetTimes {
    fn drop(&mut self) {
        let _ = crate::reset_times(self.dir_path.as_c_str(), &self.metadata);
    }
}

fn walk_dir_over(
    path: &Path,
    ignored_dirs: Arc<HashSet<PathBuf>>,
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
                    if entry.file_type().is_dir() && ignored_dirs.contains(entry.path().as_path()) {
                        return false;
                    }
                    #[allow(clippy::filetype_is_file)]
                    if entry.file_type().is_file() {
                        let reset_times = match &mut reset_times {
                            Some(reset_times) => reset_times,
                            None => reset_times.insert(
                                path.metadata()
                                    .ok()
                                    .map(|metadata| Arc::new(ResetTimes::new(path, metadata))),
                            ),
                        };
                        entry.client_state.clone_from(&reset_times);
                    }
                }
                true
            });
        },
    )
}

type State = Option<Arc<ResetTimes>>;

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
        tmpdirs: &TmpdirPaths,
        f: impl Fn(FileType, PathBuf, Option<Arc<ResetTimes>>) + Send + Sync,
    ) {
        let ignored_dirs: Arc<HashSet<PathBuf>> =
            Arc::new(tmpdirs.paths().map(PathBuf::from).collect());
        for path in self.paths {
            let walker = walk_dir_over(path, Arc::clone(&ignored_dirs));
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
