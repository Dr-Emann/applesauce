use crate::threads::{compressing, writer, BgWork, Context, WorkHandler};
use crate::{seq_queue, try_read_all, ForceWritableFile, BLOCK_SIZE};
use std::fs::{File, Metadata};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::{io, mem, thread};

pub(super) struct WorkItem {
    pub context: Arc<Context>,
    pub metadata: Metadata,
}

pub(super) struct Work {
    pub compressor: compressing::Sender,
    pub writer: writer::Sender,
}

impl BgWork for Work {
    type Item = WorkItem;
    type Handler = Handler;
    const NAME: &'static str = "reader";

    fn make_handler(&self) -> Self::Handler {
        Handler::new(self.compressor.clone(), self.writer.clone())
    }

    fn queue_capacity(&self) -> usize {
        // Allow quite a few queued up paths, to allow the total progress bar to be accurate
        100 * 1024
    }
}

pub(super) struct Handler {
    buf: Box<[u8; BLOCK_SIZE]>,
    compressor: compressing::Sender,
    writer: writer::Sender,
}

impl Handler {
    fn new(compressor: compressing::Sender, writer: writer::Sender) -> Self {
        let buf = vec![0; BLOCK_SIZE].into_boxed_slice().try_into().unwrap();
        Self {
            buf,
            compressor,
            writer,
        }
    }

    fn try_handle(&mut self, item: WorkItem) -> io::Result<()> {
        let WorkItem { context, metadata } = item;
        let path: &Path = &context.path;

        let file_size = metadata.len();
        let file = Arc::new(ForceWritableFile::open(path, &metadata)?);
        let (tx, rx) = seq_queue::bounded(
            thread::available_parallelism()
                .map(NonZeroUsize::get)
                .unwrap_or(4),
        );

        self.writer
            .send(writer::WorkItem {
                context: Arc::clone(&context),
                file: Arc::clone(&file),
                blocks: rx,
                metadata,
            })
            .unwrap();

        if let Err(mut e) = self.read_file_into(&context, &file, file_size, &tx) {
            if let Some(slot) = tx.prepare_send() {
                let orig_e = mem::replace(&mut e, io::ErrorKind::Other.into());
                let _ = slot.finish(Err(orig_e));
            }
            return Err(e);
        }

        Ok(())
    }

    fn read_file_into(
        &mut self,
        context: &Arc<Context>,
        file: &File,
        expected_len: u64,
        tx: &seq_queue::Sender<io::Result<writer::Chunk>>,
    ) -> io::Result<()> {
        let mut total_read = 0;
        let block_span = tracing::debug_span!("reading blocks");
        loop {
            let _enter = block_span.enter();

            let slot = {
                let _enter = tracing::debug_span!("waiting for free slot").entered();
                tx.prepare_send().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Other, "error must have occurred writing")
                })?
            };
            let n = try_read_all(file, &mut *self.buf)?;
            if n == 0 {
                break;
            }
            total_read += u64::try_from(n).unwrap();
            if total_read > expected_len {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "file size changed while reading",
                ));
            }

            {
                let _enter = tracing::debug_span!("waiting to send to compressor").entered();
                self.compressor
                    .send(compressing::WorkItem {
                        context: Arc::clone(context),
                        data: self.buf[..n].to_vec(),
                        slot,
                    })
                    .unwrap();
            }
        }
        if total_read != expected_len {
            // TODO: The writer doesn't know!
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "file size changed while reading",
            ));
        }
        Ok(())
    }
}

impl WorkHandler<WorkItem> for Handler {
    fn handle_item(&mut self, item: WorkItem) {
        if let Err(e) = self.try_handle(item) {
            tracing::error!("unable to compress file: {}", e);
        }
    }
}
