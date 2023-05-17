use crate::info::FileCompressionState;
use crate::progress::{self, Progress, SkipReason};
use crate::{info, scan, Stats};
use applesauce_core::compressor;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::{fmt, mem};

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

#[derive(Debug)]
pub struct OperationContext {
    mode: Mode,
    stats: Stats,
    finished_stats: crossbeam_channel::Sender<Stats>,
    verify: bool,
}

impl OperationContext {
    fn new(mode: Mode, finished_stats: crossbeam_channel::Sender<Stats>, verify: bool) -> Self {
        Self {
            mode,
            stats: Stats::default(),
            finished_stats,
            verify,
        }
    }
}

impl Drop for OperationContext {
    fn drop(&mut self) {
        let stats = mem::take(&mut self.stats);
        let _ = self.finished_stats.send(stats);
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
        let writer = BgWorker::new(16, &writer::Work);
        let reader = BgWorker::new(
            8,
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

    pub fn scan<'a, P>(
        &self,
        mode: Mode,
        paths: impl IntoIterator<Item = &'a Path>,
        progress: &P,
        verify: bool,
    ) -> Stats
    where
        P: Progress + Send + Sync,
        P::Task: Send + Sync + 'static,
    {
        let (finished_stats, finished_stats_rx) = crossbeam_channel::bounded(1);
        let operation = Arc::new(OperationContext::new(mode, finished_stats, verify));
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
        });
        drop(operation);

        finished_stats_rx
            .recv()
            .expect("OperationContext will send stats on drop of all arcs")
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
