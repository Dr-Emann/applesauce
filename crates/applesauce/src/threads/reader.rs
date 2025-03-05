use crate::seq_queue::Slot;
use crate::threads::{compressing, writer, BgWork, Context, Mode, WorkHandler};
use crate::{rfork_storage, seq_queue, try_read_all};
use applesauce_core::BLOCK_SIZE;
use std::fs::File;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::{io, thread};

pub(super) struct WorkItem {
    pub context: Arc<Context>,
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
    compressor: compressing::Sender,
    writer: writer::Sender,
}

impl Handler {
    fn new(compressor: compressing::Sender, writer: writer::Sender) -> Self {
        Self { compressor, writer }
    }

    fn read_file_into(
        &mut self,
        context: &Arc<Context>,
        file: &File,
        expected_len: u64,
        tx: &seq_queue::Sender<writer::Chunk, io::Error>,
    ) -> io::Result<()> {
        match context.operation.mode {
            Mode::Compress { kind, .. } => {
                let compressor = self.compressor.clone();
                self.with_file_chunks(file, expected_len, tx, |slot, data| {
                    let _enter = tracing::debug_span!("waiting to send to compressor").entered();
                    compressor
                        .send(compressing::WorkItem {
                            context: Arc::clone(context),
                            data,
                            slot,
                            kind,
                        })
                        .unwrap();
                    Ok(())
                })?;
            }
            Mode::DecompressManually => {
                rfork_storage::with_compressed_blocks(file, |kind| {
                    move |data| {
                        // TODO: This waits for a slot after we have already read.
                        // TODO: This should be able to exit early, without an error
                        let slot = tx
                            .prepare_send()
                            .ok_or_else(|| io::Error::other("error must have occurred writing"))?;
                        let _enter =
                            tracing::debug_span!("waiting to send to compressor").entered();
                        self.compressor
                            .send(compressing::WorkItem {
                                context: Arc::clone(context),
                                data: data.to_vec(),
                                slot,
                                kind,
                            })
                            .unwrap();
                        Ok(())
                    }
                })?;
            }
            Mode::DecompressByReading => {
                self.with_file_chunks(file, expected_len, tx, |slot, data| {
                    let orig_size = data.len() as u64;
                    let res = slot.finish(writer::Chunk {
                        block: data,
                        orig_size,
                    });
                    if let Err(e) = res {
                        // This should only happen if the writer had an error
                        tracing::debug!("error finishing chunk: {e}");
                    }
                    Ok(())
                })?;
            }
        }

        Ok(())
    }

    // return true if reading succeeded, false if the writer closed the channel
    fn with_file_chunks(
        &mut self,
        file: &File,
        expected_len: u64,
        tx: &seq_queue::Sender<writer::Chunk, io::Error>,
        mut f: impl FnMut(Slot<writer::Chunk, io::Error>, Vec<u8>) -> io::Result<()>,
    ) -> io::Result<bool> {
        let mut total_read = 0;
        let block_span = tracing::debug_span!("reading blocks");
        loop {
            let _enter = block_span.enter();

            // make sure we don't reserve a slot if we won't be sending a chunk
            if total_read == expected_len {
                let mut buf = [0];
                let n = try_read_all(file, &mut buf)?;
                total_read += u64::try_from(n).unwrap();
                // Outside the loop, we'll error if we read more than expected_len
                break;
            }

            let slot = {
                let _enter = tracing::debug_span!("waiting for free slot").entered();
                match tx.prepare_send() {
                    Some(slot) => slot,
                    None => return Ok(false),
                }
            };

            #[allow(clippy::uninit_vec)]
            // SAFETY: we just allocated this with capacity, and we will truncate it before
            //         allowing it to escape. This is not technically safe, but there's no
            //         io api that lets us use an uninit buffer yet. However, file is a
            //         std::io::File, which won't do wonky things in read, and won't lie about
            //         the return value.
            let buf = unsafe {
                let mut buf = Vec::with_capacity(BLOCK_SIZE);
                buf.set_len(BLOCK_SIZE);

                let n = try_read_all(file, &mut buf)?;
                if n == 0 {
                    break;
                }
                total_read += u64::try_from(n).unwrap();
                if total_read > expected_len {
                    return Err(io::Error::other("file size changed while reading"));
                }
                buf.truncate(n);
                buf
            };

            f(slot, buf)?;
        }
        if total_read != expected_len {
            // The writer will be notified by returning an error
            return Err(io::Error::other("file size changed while reading"));
        }
        Ok(true)
    }
}

impl WorkHandler<WorkItem> for Handler {
    fn handle_item(&mut self, item: WorkItem) {
        let WorkItem { context } = item;
        let _guard = tracing::info_span!("reading file", path=%context.path.display()).entered();
        let file = match File::open(&context.path) {
            Ok(file) => file,
            Err(e) => {
                context
                    .progress
                    .error(&format!("Error opening {}: {}", context.path.display(), e));
                return;
            }
        };
        let file = Arc::new(file);

        let file_size = context.orig_metadata.len();
        let (tx, rx) = seq_queue::bounded(
            thread::available_parallelism()
                .map(NonZeroUsize::get)
                .unwrap_or(4),
        );

        {
            let _enter = tracing::debug_span!("waiting for space in writer").entered();
            self.writer
                .send(writer::WorkItem {
                    context: Arc::clone(&context),
                    file: Arc::clone(&file),
                    blocks: rx,
                })
                .unwrap();
        }

        let result = self.read_file_into(&context, &file, file_size, &tx);
        // ensure the file is dropped before tx is finished
        drop(file);
        if let Err(e) = &result {
            context
                .progress
                .error(&format!("Error reading {}: {}", context.path.display(), e));
        }
        tx.finish(result);
    }
}
