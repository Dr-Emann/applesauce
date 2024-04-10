use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::Metadata;
use std::io;
use std::os::macos::fs::MetadataExt;
use std::path::Path;
use tempfile::{NamedTempFile, TempDir};

const TEMPDIR_PREFIX: &str = "applesauce_tmp";
const TEMPFILE_PREFIX: &str = "applesauce_tmp";

#[derive(Debug)]
pub struct TmpdirPaths {
    /// Map from device to temp dir
    dirs: HashMap<u64, TempDir>,
}

impl TmpdirPaths {
    pub fn new() -> Self {
        let mut dirs = HashMap::new();
        let system = TempDir::with_prefix(TEMPDIR_PREFIX);
        match system {
            Ok(system) => match system.path().metadata() {
                Ok(system_metadata) => {
                    dirs.insert(system_metadata.st_dev(), system);
                }
                Err(e) => {
                    tracing::warn!("failed to get metadata for system temp dir: {e}");
                }
            },
            Err(e) => {
                tracing::warn!("failed to create temp dir in system temp dir: {e}");
            }
        }

        Self { dirs }
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> + '_ {
        self.dirs.values().map(|dir| dir.path())
    }

    pub fn add_dst(&mut self, dst: &Path, metadata: &Metadata) -> io::Result<()> {
        let device = metadata.st_dev();
        match self.dirs.entry(device) {
            Entry::Occupied(_) => {}
            Entry::Vacant(entry) => {
                let tmpdir_parent = if metadata.is_dir() {
                    dst
                } else {
                    let parent = dst
                        .parent()
                        .ok_or_else(|| io::Error::other("path to file has no parent?"))?;

                    if parent.metadata()?.st_dev() != device {
                        return Err(io::Error::other(
                            "parent directory of file is on a different device?",
                        ));
                    }

                    parent
                };
                let dir = TempDir::with_prefix_in(TEMPDIR_PREFIX, tmpdir_parent)?;
                entry.insert(dir);
            }
        }
        Ok(())
    }

    pub fn tempfile_for(&self, path: &Path, metadata: &Metadata) -> io::Result<NamedTempFile> {
        let device = metadata.st_dev();
        let dir = match self.dirs.get(&device) {
            Some(dir) => dir.path(),
            None => {
                let parent = path
                    .parent()
                    .ok_or_else(|| io::Error::other("expected path to have a parent"))?;
                tracing::info!(
                    "no temp dir for device {device} found, creating file in {parent:?}"
                );
                parent
            }
        };

        let mut builder = tempfile::Builder::new();
        builder.prefix(TEMPFILE_PREFIX);
        if let Some(file_name) = path.file_name() {
            builder.suffix(file_name);
        }
        builder.tempfile_in(dir)
    }
}
