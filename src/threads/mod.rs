use crate::{compressor, Progress};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

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
    progress: Box<dyn Progress + Send + Sync>,
}

impl BackgroundThreads {
    pub fn new(compressor_kind: compressor::Kind) -> Self {
        let compressor_threads = thread::available_parallelism()
            .map(NonZeroUsize::get)
            .unwrap_or(1);

        let compressor = BgWorker::new(compressor_threads, &compressing::Work { compressor_kind });
        let writer = BgWorker::new(2, &writer::Work { compressor_kind });
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

    pub fn submit(&self, path: PathBuf, progress: Box<dyn Progress + Send + Sync>) {
        self.reader
            .chan()
            .send(reader::WorkItem {
                context: Arc::new(Context { path, progress }),
            })
            .unwrap()
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
                    .name(format!("{} {}", Work::NAME, i))
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
