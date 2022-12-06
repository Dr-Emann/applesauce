use crate::threads::{writer, BgWork, Context, WorkHandler};
use crate::{
    compressor::{self, Compressor},
    seq_queue, BLOCK_SIZE,
};
use std::io;
use std::sync::Arc;

pub(super) type Sender = crossbeam_channel::Sender<WorkItem>;

pub(super) struct WorkItem {
    pub context: Arc<Context>,
    pub data: Vec<u8>,
    pub slot: seq_queue::Slot<io::Result<writer::Chunk>>,
}

pub(super) struct Work {
    pub compressor_kind: compressor::Kind,
    pub compress: bool,
}

impl BgWork for Work {
    type Item = WorkItem;
    type Handler = Handler;
    const NAME: &'static str = "compressor";

    fn make_handler(&self) -> Self::Handler {
        Handler {
            compressor: self.compressor_kind.compressor().unwrap(),
            compress: self.compress,
            buf: vec![0; BLOCK_SIZE + 1024],
        }
    }

    fn queue_capacity(&self) -> usize {
        8
    }
}

pub(super) struct Handler {
    compressor: Compressor,
    compress: bool,
    buf: Vec<u8>,
}

impl WorkHandler<WorkItem> for Handler {
    fn handle_item(&mut self, item: WorkItem) {
        let _entered =
            tracing::debug_span!("compressing block", path=%item.context.path.display()).entered();

        let f = if self.compress {
            Compressor::compress
        } else {
            Compressor::decompress
        };
        let size = match f(&mut self.compressor, &mut self.buf, &item.data) {
            Ok(size) => size,
            Err(e) => {
                if item.slot.finish(Err(e)).is_err() {
                    // This should only be because of a failure already reported by the writer
                    tracing::debug!("unable to finish slot");
                }
                return;
            }
        };
        debug_assert!(size != 0);

        let chunk = writer::Chunk {
            block: self.buf[..size].to_vec(),
            orig_size: item.data.len().try_into().unwrap(),
        };
        if item.slot.finish(Ok(chunk)).is_err() {
            // This should only be because of a failure already reported by the writer
            tracing::debug!("unable to finish slot");
        }
    }
}
