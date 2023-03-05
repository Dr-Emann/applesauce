use crate::threads::{writer, BgWork, Context, Mode, WorkHandler};
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
    pub kind: compressor::Kind,
    pub slot: seq_queue::Slot<io::Result<writer::Chunk>>,
}

pub(super) struct Work;

impl BgWork for Work {
    type Item = WorkItem;
    type Handler = Handler;
    const NAME: &'static str = "compressor";

    fn make_handler(&self) -> Self::Handler {
        Handler {
            compressors: (0..3).map(|_| None).collect(),
            buf: vec![0; BLOCK_SIZE + 1024],
        }
    }

    fn queue_capacity(&self) -> usize {
        8
    }
}

pub(super) struct Handler {
    compressors: Vec<Option<Compressor>>,
    buf: Vec<u8>,
}

impl WorkHandler<WorkItem> for Handler {
    fn handle_item(&mut self, item: WorkItem) {
        let _entered =
            tracing::debug_span!("compressing block", path=%item.context.path.display()).entered();

        // TODO: Unwrap?
        let compressor = self.compressors[item.kind as usize]
            .get_or_insert_with(|| item.kind.compressor().unwrap());
        let size = match item.context.mode {
            Mode::Compress { kind, level } => {
                debug_assert_eq!(kind, item.kind);
                compressor.compress(&mut self.buf, &item.data, level)
            }
            Mode::DecompressManually => compressor.decompress(&mut self.buf, &item.data),
            Mode::DecompressByReading => {
                panic!("decompressing by reading should not be using the compressor thread")
            }
        };
        let size = match size {
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
