use crate::compressor;
use std::num::NonZeroUsize;
use std::path::Path;
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
    reader: reader::ReaderThreads,
    _compressor: compressing::CompressionThreads,
    _writer: writer::WriterThreads,
}

impl BackgroundThreads {
    pub fn new(compressor_kind: compressor::Kind) -> Self {
        let compressor = compressing::CompressionThreads::new(
            compressor_kind,
            thread::available_parallelism()
                .map(NonZeroUsize::get)
                .unwrap_or(4),
        );
        let writer = writer::WriterThreads::new(2, compressor_kind);
        let reader = reader::ReaderThreads::new(2, compressor.chan(), writer.chan());
        Self {
            reader,
            _compressor: compressor,
            _writer: writer,
        }
    }

    pub fn submit(&self, path: &Path) {
        self.reader
            .chan()
            .send(reader::WorkItem { path: path.into() })
            .unwrap()
    }
}
