use crate::theads::{compressing, writer, ThreadJoiner};
use crate::{check_compressible, seq_queue, try_read_all, ForceWritableFile, BLOCK_SIZE};
use std::fs::File;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::{io, mem, thread};

pub type Sender = crossbeam_channel::Sender<WorkItem>;

pub struct WorkItem {
    pub path: Arc<Path>,
}

pub struct ReaderThreads {
    // Order is important: Drop happens top to bottom, drop the sender before trying to join the threads
    tx: crossbeam_channel::Sender<WorkItem>,
    _joiner: ThreadJoiner,
}

impl ReaderThreads {
    pub fn new(
        count: usize,
        compressor_chan: &compressing::Sender,
        writer_chan: &writer::Sender,
    ) -> Self {
        assert!(count > 0);

        let (tx, rx) = crossbeam_channel::bounded(1);
        let threads: Vec<_> = (0..count)
            .map(|_| {
                let rx = rx.clone();
                let compressor_chan = compressor_chan.clone();
                let writer_chan = writer_chan.clone();
                thread::spawn(move || thread_impl(rx, compressor_chan, writer_chan))
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

struct Reader {
    buf: Box<[u8; BLOCK_SIZE]>,
    compressor: compressing::Sender,
    writer: writer::Sender,
}

impl Reader {
    fn new(compressor: compressing::Sender, writer: writer::Sender) -> Self {
        let buf = vec![0; BLOCK_SIZE].into_boxed_slice().try_into().unwrap();
        Self {
            buf,
            compressor,
            writer,
        }
    }

    fn handle_work_item(&mut self, item: WorkItem) -> io::Result<()> {
        let WorkItem { path } = item;
        let metadata = path.metadata()?;

        check_compressible(&path, &metadata)?;

        let file_size = metadata.len();
        let file = Arc::new(ForceWritableFile::open(&path, &metadata)?);
        let (tx, rx) = seq_queue::bounded(
            thread::available_parallelism()
                .map(NonZeroUsize::get)
                .map(|x| x * 2)
                .unwrap_or(4),
        );

        self.writer
            .send(writer::WorkItem {
                path: Arc::clone(&path),
                file: Arc::clone(&file),
                blocks: rx,
                metadata,
            })
            .unwrap();

        if let Err(mut e) = self.read_file_into(&path, &file, file_size, &tx) {
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
        path: &Arc<Path>,
        file: &File,
        expected_len: u64,
        tx: &seq_queue::Sender<io::Result<Vec<u8>>>,
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
                        path: Arc::clone(path),
                        data: self.buf[..n].to_vec(),
                        slot,
                    })
                    .unwrap();
            }
        }
        if total_read != expected_len {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "file size changed while reading",
            ));
        }
        Ok(())
    }
}

fn thread_impl(
    rx: crossbeam_channel::Receiver<WorkItem>,
    compressor_chan: compressing::Sender,
    writer_chan: writer::Sender,
) {
    let _entered = tracing::debug_span!("reader thread").entered();
    let mut reader = Reader::new(compressor_chan, writer_chan);
    for item in rx {
        let _entered = tracing::info_span!("reading file", path=%item.path.display()).entered();
        if let Err(e) = reader.handle_work_item(item) {
            tracing::error!("unable to compress file: {}", e);
        }
    }
}
