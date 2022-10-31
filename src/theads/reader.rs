use crate::seq_queue;
use crate::theads::ThreadJoiner;
use std::path::PathBuf;

pub struct WorkItem {
    path: PathBuf,
}

pub struct ReaderThreads {
    // Order is important: Drop happens top to bottom, drop the sender before trying to join the threads
    tx: crossbeam_channel::Sender<WorkItem>,
    joiner: ThreadJoiner,
}

impl CompressionThreads {
    pub fn new(compressor_kind: compressor::Kind, count: usize) -> Self {
        assert!(count > 0);

        let (tx, rx) = crossbeam_channel::bounded(1);
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
}

fn thread_impl(compressor_kind: compressor::Kind, rx: crossbeam_channel::Receiver<WorkItem>) {
    let mut compressor = compressor_kind.compressor().unwrap();

    for item in rx {
        let mut dst = vec![0; BLOCK_SIZE + 1024];
        let size = compressor.compress(&mut dst, &item.data).unwrap();
        dst.truncate(size);
        item.slot.finish(dst).unwrap();
    }
}
