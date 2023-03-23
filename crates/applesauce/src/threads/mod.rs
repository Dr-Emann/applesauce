use crate::info::{FileCompressionState, FileInfo};
use crate::progress::{self, Progress, SkipReason};
use crate::{info, scan};
use applesauce_core::compressor;
use std::fmt;
use std::fs::Metadata;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub mod compressing;
pub mod reader;
pub mod writer;

struct ThreadJoiner {
    threads: Vec<JoinHandle<()>>,
}

impl ThreadJoiner {
    fn new(threads: Vec<JoinHandle<()>>) -> Self {
        Self { threads }
    }
}

impl Drop for ThreadJoiner {
    fn drop(&mut self) {
        for handle in self.threads.drain(..) {
            handle.join().unwrap();
        }
    }
}

pub struct BackgroundThreads {
    reader: BgWorker<reader::Work>,
    _compressor: BgWorker<compressing::Work>,
    _writer: BgWorker<writer::Work>,
}

#[derive(Debug, Default)]
pub struct Stats {
    /// Total number of files scanned
    pub files: AtomicU64,
    /// Total of all file sizes (uncompressed)
    pub total_file_sizes: AtomicU64,

    pub compressed_size_start: AtomicU64,
    /// Total of all file sizes (after compression) after performing this operation
    pub compressed_size_final: AtomicU64,
    /// Number of files that were compressed before performing this operation
    pub compressed_file_count_start: AtomicU64,
    /// Number of files that were compressed after performing this operation
    pub compressed_file_count_final: AtomicU64,

    /// Number of files that were incompressible (only present when compressing)
    pub incompressible_file_count: AtomicU64,
}

impl Stats {
    fn add_start_file(&self, metadata: &Metadata, file_info: &FileInfo) {
        self.files
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.total_file_sizes
            .fetch_add(metadata.len(), std::sync::atomic::Ordering::Relaxed);
        self.compressed_size_start
            .fetch_add(file_info.on_disk_size, std::sync::atomic::Ordering::Relaxed);
        match file_info.compression_state {
            FileCompressionState::Compressed => {
                self.compressed_file_count_start
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            FileCompressionState::Compressible => {}
            FileCompressionState::Incompressible(_) => {
                self.incompressible_file_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    fn add_end_file(&self, _metadata: &Metadata, file_info: &FileInfo) {
        self.compressed_size_final
            .fetch_add(file_info.on_disk_size, std::sync::atomic::Ordering::Relaxed);
        if let FileCompressionState::Compressed = file_info.compression_state {
            self.compressed_file_count_final
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[derive(Debug)]
pub struct OperationContext {
    mode: Mode,
    stats: Stats,
}

impl OperationContext {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            stats: Stats::default(),
        }
    }
}

pub struct Context {
    operation: Arc<OperationContext>,
    path: PathBuf,
    orig_size: u64,
    progress: Box<dyn progress::Task + Send + Sync>,
}

impl Drop for Context {
    fn drop(&mut self) {
        let Ok(metadata) = self.path.symlink_metadata() else { return };
        let file_info = info::get_file_info(&self.path, &metadata);
        self.operation.stats.add_end_file(&metadata, &file_info);
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Mode {
    Compress {
        kind: compressor::Kind,
        minimum_compression_ratio: f64,
        level: u32,
    },
    DecompressManually,
    DecompressByReading,
}

impl Mode {
    pub fn is_compressing(self) -> bool {
        matches!(self, Self::Compress { .. })
    }
}

impl BackgroundThreads {
    #[must_use]
    pub fn new() -> Self {
        let compressor_threads = thread::available_parallelism()
            .map(NonZeroUsize::get)
            .unwrap_or(1);

        let compressor = BgWorker::new(compressor_threads, &compressing::Work);
        let writer = BgWorker::new(4, &writer::Work);
        let reader = BgWorker::new(
            2,
            &reader::Work {
                compressor: compressor.chan().clone(),
                writer: writer.chan().clone(),
            },
        );
        Self {
            reader,
            _compressor: compressor,
            _writer: writer,
        }
    }

    pub fn scan<'a, P>(&self, mode: Mode, paths: impl IntoIterator<Item = &'a Path>, progress: &P)
    where
        P: Progress + Send + Sync,
        P::Task: Send + Sync + 'static,
    {
        let operation = Arc::new(OperationContext::new(mode));
        let stats = &operation.stats;
        let chan = self.reader.chan();

        let walker = scan::Walker::new(paths, progress);
        walker.run(|file_type, path| {
            // We really only want to deal with files, not symlinks to files, or fifos, etc.
            #[allow(clippy::filetype_is_file)]
            if !file_type.is_file() {
                progress.file_skipped(&path, SkipReason::NotFile);
                return;
            }
            let metadata = match path.symlink_metadata() {
                Ok(metadata) => metadata,
                Err(e) => {
                    progress.file_skipped(&path, SkipReason::ReadError(e));
                    return;
                }
            };

            let file_info = info::get_file_info(&path, &metadata);
            stats.add_start_file(&metadata, &file_info);

            let send = match file_info.compression_state {
                FileCompressionState::Compressed if !mode.is_compressing() => true,
                FileCompressionState::Compressible if mode.is_compressing() => true,
                FileCompressionState::Incompressible(_) => {
                    return;
                }
                _ => false,
            };
            if send {
                let inner_progress = Box::new(progress.file_task(&path, metadata.len()));
                chan.send(reader::WorkItem {
                    context: Arc::new(Context {
                        operation: Arc::clone(&operation),
                        path,
                        progress: inner_progress,
                        orig_size: metadata.len(),
                    }),
                    metadata,
                })
                .unwrap();
            } else {
                stats.add_end_file(&metadata, &file_info);
            }
        })
    }
}

impl Default for BackgroundThreads {
    fn default() -> Self {
        Self::new()
    }
}

trait WorkHandler<WorkItem> {
    fn handle_item(&mut self, item: WorkItem);
}

trait BgWork {
    type Item: Send + Sync + 'static;
    type Handler: WorkHandler<Self::Item> + Send + 'static;

    const NAME: &'static str;

    fn make_handler(&self) -> Self::Handler;
    fn queue_capacity(&self) -> usize {
        1
    }
}

struct BgWorker<Work: BgWork> {
    tx: crossbeam_channel::Sender<Work::Item>,
    _joiner: ThreadJoiner,
}

impl<Work: BgWork> BgWorker<Work> {
    pub fn new(thread_count: usize, work: &Work) -> Self {
        assert!(thread_count > 0);

        let (tx, rx) = crossbeam_channel::bounded(work.queue_capacity());
        let threads: Vec<_> = (0..thread_count)
            .map(|i| {
                let rx = rx.clone();
                let handler = work.make_handler();

                thread::Builder::new()
                    .name(format!("{} {i}", Work::NAME))
                    .spawn(move || handle_fn(rx, handler))
                    .unwrap()
            })
            .collect();

        Self {
            tx,
            _joiner: ThreadJoiner::new(threads),
        }
    }

    pub fn chan(&self) -> &crossbeam_channel::Sender<Work::Item> {
        &self.tx
    }
}

fn handle_fn<WorkItem, Handler: WorkHandler<WorkItem>>(
    rx: crossbeam_channel::Receiver<WorkItem>,
    mut handler: Handler,
) {
    for item in rx {
        handler.handle_item(item);
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("path", &self.path)
            .field("orig_size", &self.orig_size)
            .field("operation", &self.operation)
            .finish()
    }
}
