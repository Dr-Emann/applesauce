use crate::decmpfs::CompressionType;
use crate::threads::{BgWork, Context, WorkHandler};
use crate::{
    compressor, decmpfs, num_blocks, reset_times, seq_queue, set_flags, xattr, ForceWritableFile,
};
use resource_fork::ResourceFork;
use std::fs::{File, Metadata};
use std::io;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::os::macos::fs::MetadataExt;
use std::sync::Arc;

pub(super) type Sender = crossbeam_channel::Sender<WorkItem>;

pub(super) struct Chunk {
    pub block: Vec<u8>,
    pub orig_size: u64,
}

pub(super) struct WorkItem {
    pub context: Arc<Context>,
    pub(crate) file: Arc<ForceWritableFile>,
    pub blocks: seq_queue::Receiver<io::Result<Chunk>>,
    pub metadata: Metadata,
}

pub(super) struct Work {
    pub compressor_kind: compressor::Kind,
}

impl BgWork for Work {
    type Item = WorkItem;
    type Handler = Handler;
    const NAME: &'static str = "writer";

    fn make_handler(&self) -> Handler {
        Handler::new(self.compressor_kind)
    }
}

enum WriteDest {
    Xattr { data: Vec<u8> },
    BlocksWritten,
}

pub(super) struct Handler {
    compressor_kind: compressor::Kind,
    decomp_xattr_val_buf: Vec<u8>,
    block_sizes: Vec<u32>,
}

impl Handler {
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

        let mut total_compressed_size = 0;
        let mut add_compressed_chunk = |chunk: &Chunk| -> io::Result<()> {
            total_compressed_size += u64::try_from(chunk.block.len()).unwrap();
            if total_compressed_size > context.orig_size {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "file grew when compressed",
                ));
            }
            Ok(())
        };
        let chunk1 = match chunks.recv() {
            Ok(chunk) => chunk?,
            Err(_) => return Ok(WriteDest::Xattr { data: Vec::new() }),
        };
        let chunk2 = match chunks.recv() {
            Ok(chunk) => chunk?,
            Err(_) => {
                // no second block
                add_compressed_chunk(&chunk1)?;
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
            add_compressed_chunk(&chunk1)?;
            self.block_sizes
                .push(chunk1.block.len().try_into().unwrap());
            writer.write_all(&chunk1.block)?;
            context.progress.increment(chunk1.orig_size);
        }
        {
            let _enter = block_span.enter();
            add_compressed_chunk(&chunk2)?;
            self.block_sizes
                .push(chunk2.block.len().try_into().unwrap());
            writer.write_all(&chunk2.block)?;
            context.progress.increment(chunk2.orig_size);
        }
        drop((chunk1, chunk2));

        for chunk in chunks {
            let chunk = chunk?;
            add_compressed_chunk(&chunk)?;

            let Chunk { block, orig_size } = chunk;
            let _enter = block_span.enter();

            self.block_sizes.push(block.len().try_into().unwrap());
            writer.write_all(&block)?;
            context.progress.increment(orig_size);
        }
        Ok(WriteDest::BlocksWritten)
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

        let compression_type = CompressionType::new(self.compressor_kind, storage);
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
        xattr::set(file, decmpfs::XATTR_NAME, &self.decomp_xattr_val_buf, 0)?;
        Ok(())
    }
}

impl WorkHandler<WorkItem> for Handler {
    fn handle_item(&mut self, item: WorkItem) {
        let _entered =
            tracing::info_span!("writing file", path=%item.context.path.display()).entered();
        let file_size = item.metadata.len();
        let block_count: u32 = num_blocks(file_size).try_into().unwrap();

        self.block_sizes.clear();
        self.block_sizes.reserve(block_count.try_into().unwrap());

        let mut resource_fork =
            BufWriter::with_capacity(crate::BLOCK_SIZE, ResourceFork::new(&item.file));
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
            if let Err(e) = xattr::remove(&item.file, decmpfs::XATTR_NAME) {
                tracing::error!("error while removing decmpfs header: {}", e);
            }
            if let Err(e) = xattr::remove(&item.file, resource_fork::XATTR_NAME) {
                tracing::error!("error while removing resource fork: {}", e);
            }
        }

        if let Err(e) = reset_times(&item.file, &item.metadata) {
            tracing::error!("unable to reset file times: {}", e);
        }

        if res.is_ok() {
            tracing::info!("Successfully compressed {}", item.context.path.display());
        }
    }
}
