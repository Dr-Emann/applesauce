use crate::threads::{writer, Context, ThreadJoiner};
use crate::{compressor, seq_queue, BLOCK_SIZE};
use std::sync::Arc;
use std::{io, thread};

pub(super) type Sender = crossbeam_channel::Sender<WorkItem>;

pub(super) struct WorkItem {
    pub context: Arc<Context>,
    pub data: Vec<u8>,
    pub slot: seq_queue::Slot<io::Result<writer::Chunk>>,
}

pub(super) struct CompressionThreads {
    // Order is important: Drop happens top to bottom, drop the sender before trying to join the threads
    tx: crossbeam_channel::Sender<WorkItem>,
    _joiner: ThreadJoiner,
}

impl CompressionThreads {
    pub fn new(count: usize, compressor_kind: compressor::Kind) -> Self {
        assert!(count > 0);

        let (tx, rx) = crossbeam_channel::bounded(8);
        let threads: Vec<_> = (0..count)
            .map(|i| {
                let rx = rx.clone();

                thread::Builder::new()
                    .name(format!("compressor {i}"))
                    .spawn(move || thread_impl(compressor_kind, rx))
                    .unwrap()
            })
            .collect();

        Self {
            tx,
            _joiner: ThreadJoiner::new(threads),
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
            tracing::info_span!("compressing block", path=%item.context.path.display()).entered();
        let size = compressor.compress(&mut buf, &item.data).unwrap();
        debug_assert!(size != 0);

        let chunk = writer::Chunk {
            block: buf[..size].to_vec(),
            orig_size: item.data.len().try_into().unwrap(),
        };
        if item.slot.finish(Ok(chunk)).is_err() {
            // This should only be because of a failure already reported by the writer
            tracing::debug!("unable to finish slot");
        }
    }
}
