use crate::decmpfs::CompressionType;
use crate::resource_fork::ResourceFork;
use crate::theads::{writer, Context, ThreadJoiner};
use crate::{
    compressor, decmpfs, num_blocks, remove_xattr, reset_times, resource_fork, seq_queue,
    set_flags, set_xattr, ForceWritableFile,
};
use std::fs::{File, Metadata};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::os::macos::fs::MetadataExt;
use std::sync::Arc;
use std::{io, thread};

pub type Sender = crossbeam_channel::Sender<WorkItem>;

pub struct Chunk {
    pub block: Vec<u8>,
    pub orig_size: u64,
}

pub struct WorkItem {
    pub context: Arc<Context>,
    pub(crate) file: Arc<ForceWritableFile>,
    pub blocks: seq_queue::Receiver<io::Result<Chunk>>,
    pub metadata: Metadata,
}

pub struct WriterThreads {
    // Order is important: Drop happens top to bottom, drop the sender before trying to join the threads
    tx: crossbeam_channel::Sender<WorkItem>,
    _joiner: ThreadJoiner,
}

impl WriterThreads {
    pub fn new(count: usize, compressor_kind: compressor::Kind) -> Self {
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
            _joiner: ThreadJoiner::new(threads),
        }
    }

    pub fn chan(&self) -> &Sender {
        &self.tx
    }
}

enum WriteDest {
    Xattr { data: Vec<u8> },
    BlocksWritten,
}

struct Writer {
    compressor_kind: compressor::Kind,
    decomp_xattr_val_buf: Vec<u8>,
    block_sizes: Vec<u32>,
}

impl Writer {
    fn new(compressor_kind: compressor::Kind) -> Self {
        Self {
            compressor_kind,
            decomp_xattr_val_buf: Vec::with_capacity(decmpfs::MAX_XATTR_SIZE),
            block_sizes: Vec::new(),
        }
    }

    #[tracing::instrument(level = "debug", skip_all, err)]
    fn write_blocks<W: Write>(
        &mut self,
        context: &Context,
        mut writer: W,
        chunks: seq_queue::Receiver<io::Result<Chunk>>,
    ) -> io::Result<WriteDest> {
        let block_span = tracing::debug_span!("write block");

        let chunk1 = match chunks.recv() {
            Ok(chunk) => chunk?,
            Err(_) => return Ok(WriteDest::Xattr { data: Vec::new() }),
        };
        let chunk2 = match chunks.recv() {
            Ok(chunk) => chunk?,
            Err(_) => {
                // no second block
                let result = if chunk1.block.len() <= decmpfs::MAX_XATTR_DATA_SIZE {
                    tracing::debug!("compressible inside xattr");
                    WriteDest::Xattr { data: chunk1.block }
                } else {
                    let _enter = block_span.enter();
                    self.block_sizes
                        .push(chunk1.block.len().try_into().unwrap());
                    writer.write_all(&chunk1.block)?;
                    WriteDest::BlocksWritten
                };
                context.progress.increment(chunk1.orig_size);
                return Ok(result);
            }
        };
        {
            let _enter = block_span.enter();
            self.block_sizes
                .push(chunk1.block.len().try_into().unwrap());
            writer.write_all(&chunk1.block)?;
            context.progress.increment(chunk1.orig_size);
        }
        {
            let _enter = block_span.enter();
            self.block_sizes
                .push(chunk2.block.len().try_into().unwrap());
            writer.write_all(&chunk2.block)?;
            context.progress.increment(chunk2.orig_size);
        }
        drop((chunk1, chunk2));

        for chunk in chunks {
            let Chunk { block, orig_size } = chunk?;
            let _enter = block_span.enter();

            self.block_sizes.push(block.len().try_into().unwrap());
            writer.write_all(&block)?;
            context.progress.increment(orig_size);
        }
        Ok(WriteDest::BlocksWritten)
    }

    fn handle_work_item(&mut self, item: WorkItem) {
        let file_size = item.metadata.len();
        let block_count: u32 = num_blocks(file_size).try_into().unwrap();

        self.block_sizes.clear();
        self.block_sizes.reserve(block_count.try_into().unwrap());

        let mut resource_fork = BufWriter::new(ResourceFork::new(&item.file));
        resource_fork
            .seek(SeekFrom::Start(
                self.compressor_kind.blocks_start(block_count.into()),
            ))
            .unwrap();

        let res = self
            .write_blocks(&item.context, &mut resource_fork, item.blocks)
            .and_then(|write_dest| {
                self.finish_xattrs(&item.file, file_size, write_dest, resource_fork)
            })
            .and_then(|()| {
                let _entered = tracing::trace_span!("truncating file").entered();
                // TODO: Decompress back into file on error
                item.file.set_len(0)
            })
            .and_then(|()| set_flags(&item.file, item.metadata.st_flags() | libc::UF_COMPRESSED));

        if res.is_err() {
            let _enter = tracing::error_span!("removing xattrs").entered();
            if let Err(e) = remove_xattr(&item.file, decmpfs::XATTR_NAME) {
                tracing::error!("error while removing decmpfs header: {}", e);
            }
            if let Err(e) = remove_xattr(&item.file, resource_fork::XATTR_NAME) {
                tracing::error!("error while removing resource fork: {}", e);
            }
        }

        if let Err(e) = reset_times(&item.file, &item.metadata) {
            tracing::error!("unable to reset file times: {}", e);
        }
    }

    #[tracing::instrument(level = "debug", skip_all, err)]
    fn finish_xattrs<W: Write + Seek>(
        &mut self,
        file: &File,
        file_size: u64,
        write_dest: WriteDest,
        mut resource_fork: W,
    ) -> io::Result<()> {
        let (storage, extra_data) = match write_dest {
            WriteDest::Xattr { data } => (decmpfs::Storage::Xattr, data),
            WriteDest::BlocksWritten => (decmpfs::Storage::ResourceFork, Vec::new()),
        };

        let compression_type = CompressionType {
            compression: self.compressor_kind,
            storage,
        };
        let header = decmpfs::DiskHeader {
            compression_type: compression_type.raw_type(),
            uncompressed_size: file_size,
        };

        self.decomp_xattr_val_buf.clear();
        self.decomp_xattr_val_buf
            .reserve(decmpfs::DiskHeader::SIZE + extra_data.len());
        header.write_into(&mut self.decomp_xattr_val_buf)?;

        if storage == decmpfs::Storage::ResourceFork {
            self.compressor_kind
                .finish(&mut resource_fork, &self.block_sizes)?;
            resource_fork.flush()?;
        } else {
            self.decomp_xattr_val_buf.extend_from_slice(&extra_data);
        }
        set_xattr(file, decmpfs::XATTR_NAME, &self.decomp_xattr_val_buf, 0)?;
        Ok(())
    }
}

fn thread_impl(compressor_kind: compressor::Kind, rx: crossbeam_channel::Receiver<WorkItem>) {
    let _entered = tracing::debug_span!("writer thread").entered();
    let mut writer = Writer::new(compressor_kind);
    for item in rx {
        let _entered =
            tracing::info_span!("writing file", path=%item.context.path.display()).entered();
        writer.handle_work_item(item);
    }
}
