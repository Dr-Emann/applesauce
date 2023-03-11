use crate::progress::{self, Progress};
use crate::{compressor, scan};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
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

pub struct Context {
    path: PathBuf,
    orig_size: u64,
    progress: Box<dyn progress::Task + Send + Sync>,
    mode: Mode,
}

#[derive(Debug, Copy, Clone)]
pub enum Mode {
    Compress { kind: compressor::Kind, level: u32 },
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
        let chan = self.reader.chan();
        let walker = scan::Walker::new(paths, progress);
        walker.run(mode, |path, metadata| {
            let progress = Box::new(progress.file_task(&path, metadata.len()));
            chan.send(reader::WorkItem {
                context: Arc::new(Context {
                    path,
                    progress,
                    mode,
                    orig_size: metadata.len(),
                }),
                metadata,
            })
            .unwrap();
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
