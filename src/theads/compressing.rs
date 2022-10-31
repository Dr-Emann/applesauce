use crate::theads::ThreadJoiner;
use crate::{compressor, seq_queue, BLOCK_SIZE};
use std::path::Path;
use std::sync::Arc;
use std::{io, thread};

pub type Sender = crossbeam_channel::Sender<WorkItem>;

pub struct WorkItem {
    pub path: Arc<Path>,
    pub data: Vec<u8>,
    pub slot: seq_queue::Slot<io::Result<Vec<u8>>>,
}

pub struct CompressionThreads {
    // Order is important: Drop happens top to bottom, drop the sender before trying to join the threads
    tx: crossbeam_channel::Sender<WorkItem>,
    joiner: ThreadJoiner,
}

impl CompressionThreads {
    pub fn new(compressor_kind: compressor::Kind, count: usize) -> Self {
        assert!(count > 0);

        let (tx, rx) = crossbeam_channel::bounded(8);
        let threads: Vec<_> = (0..count)
            .map(|_| {
                let rx = rx.clone();
                thread::spawn(move || thread_impl(compressor_kind, rx))
            })
            .collect();

        Self {
            tx,
            joiner: ThreadJoiner::new(threads),
        }
    }

    pub fn chan(&self) -> &Sender {
        &self.tx
    }
}

fn thread_impl(compressor_kind: compressor::Kind, rx: crossbeam_channel::Receiver<WorkItem>) {
    let _entered = tracing::debug_span!("compressing thread").entered();
    let mut compressor = compressor_kind.compressor().unwrap();
    let mut buf = vec![0; BLOCK_SIZE + 1024];

    for item in rx {
        let _entered =
            tracing::info_span!("compressing block", path=%item.path.display()).entered();
        let size = compressor.compress(&mut buf, &item.data).unwrap();
        if item.slot.finish(Ok(buf[..size].to_vec())).is_err() {
            // This should only be because of a failure already reported by the writer
            tracing::debug!("unable to finish slot");
        }
    }
}
