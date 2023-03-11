use crate::compressor::Kind;
use crate::decmpfs::CompressionType;
use crate::threads::{BgWork, Context, Mode, WorkHandler};
use crate::{compressor, decmpfs, num_blocks, reset_times, seq_queue, set_flags, xattr};
use resource_fork::ResourceFork;
use std::fs::{File, Metadata};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::macos::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::{io, ptr};
use tempfile::NamedTempFile;

pub(super) type Sender = crossbeam_channel::Sender<WorkItem>;

pub(super) struct Chunk {
    pub block: Vec<u8>,
    pub orig_size: u64,
}

pub(super) struct WorkItem {
    pub context: Arc<Context>,
    pub file: Arc<File>,
    pub blocks: seq_queue::Receiver<io::Result<Chunk>>,
    pub metadata: Metadata,
}

pub(super) struct Work;

impl BgWork for Work {
    type Item = WorkItem;
    type Handler = Handler;
    const NAME: &'static str = "writer";

    fn make_handler(&self) -> Handler {
        Handler::new()
    }
}

enum WriteDest {
    Xattr { data: Vec<u8> },
    BlocksWritten,
}

pub(super) struct Handler {
    decomp_xattr_val_buf: Vec<u8>,
    block_sizes: Vec<u32>,
}

impl Handler {
    fn new() -> Self {
        Self {
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
                context.progress.not_compressible_enough(&context.path);
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
        compressor_kind: compressor::Kind,
        write_dest: WriteDest,
        mut resource_fork: W,
    ) -> io::Result<()> {
        let (storage, extra_data) = match write_dest {
            WriteDest::Xattr { data } => (decmpfs::Storage::Xattr, data),
            WriteDest::BlocksWritten => (decmpfs::Storage::ResourceFork, Vec::new()),
        };

        let compression_type = CompressionType::new(compressor_kind, storage);
        let decmpfs_value = decmpfs::Value {
            compression_type,
            uncompressed_size: file_size,
            extra_data: &extra_data,
        };

        self.decomp_xattr_val_buf.clear();
        self.decomp_xattr_val_buf.reserve(decmpfs_value.len());
        // Writing to a Vec never fails
        decmpfs_value
            .write_to(&mut self.decomp_xattr_val_buf)
            .unwrap();
        xattr::set(file, decmpfs::XATTR_NAME, &self.decomp_xattr_val_buf, 0)?;

        if storage == decmpfs::Storage::ResourceFork {
            compressor_kind.finish(&mut resource_fork, &self.block_sizes)?;
            resource_fork.flush()?;
        }
        Ok(())
    }

    fn write_compressed_file(&mut self, item: WorkItem, compressor_kind: Kind) -> io::Result<()> {
        let file_size = item.metadata.len();
        let block_count: u32 = num_blocks(file_size).try_into().unwrap();

        self.block_sizes.clear();
        self.block_sizes.reserve(block_count.try_into().unwrap());

        let tmp_file = tmp_file_for(&item.context.path)?;
        copy_xattrs(&item.file, tmp_file.as_file())?;

        let mut resource_fork =
            BufWriter::with_capacity(crate::BLOCK_SIZE, ResourceFork::new(tmp_file.as_file()));
        resource_fork.seek(SeekFrom::Start(
            compressor_kind.blocks_start(block_count.into()),
        ))?;

        let write_dest = self.write_blocks(&item.context, &mut resource_fork, item.blocks)?;
        self.finish_xattrs(
            tmp_file.as_file(),
            file_size,
            compressor_kind,
            write_dest,
            resource_fork,
        )?;
        tmp_file.as_file().set_len(0)?;

        copy_metadata(&item.file, tmp_file.as_file())?;
        set_flags(
            tmp_file.as_file(),
            item.metadata.st_flags() | libc::UF_COMPRESSED,
        )?;
        tmp_file.as_file().sync_all()?;

        let new_file = tmp_file.persist(&item.context.path)?;
        if let Err(e) = reset_times(&new_file, &item.metadata) {
            tracing::error!("Unable to reset times: {e}");
        }
        Ok(())
    }

    fn write_uncompressed_file(&mut self, item: WorkItem) -> io::Result<()> {
        let mut tmp_file = tmp_file_for(&item.context.path)?;
        copy_xattrs(&item.file, tmp_file.as_file())?;

        for chunk in item.blocks {
            let chunk = chunk?;
            tmp_file.write_all(&chunk.block)?;
        }

        copy_metadata(&item.file, tmp_file.as_file())?;
        set_flags(
            tmp_file.as_file(),
            item.metadata.st_flags() & !libc::UF_COMPRESSED,
        )?;
        tmp_file.as_file().sync_all()?;

        let new_file = tmp_file.persist(&item.context.path)?;
        if let Err(e) = reset_times(&new_file, &item.metadata) {
            tracing::error!("Unable to reset times: {e}");
        }
        Ok(())
    }
}

impl WorkHandler<WorkItem> for Handler {
    fn handle_item(&mut self, item: WorkItem) {
        let context = Arc::clone(&item.context);
        let _entered = tracing::info_span!("writing file", path=%context.path.display()).entered();

        let res = match context.mode {
            Mode::Compress { kind, .. } => self.write_compressed_file(item, kind),
            Mode::DecompressManually | Mode::DecompressByReading => {
                self.write_uncompressed_file(item)
            }
        };

        if res.is_ok() {
            let compressing = context.mode.is_compressing();
            let prefix = if compressing { "" } else { "de" };
            tracing::info!("Successfully {prefix}compressed {}", context.path.display());
        }
    }
}

fn tmp_file_for(path: &Path) -> io::Result<NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    if let Some(name) = path.file_name() {
        builder.prefix(name);
    }
    builder.tempfile_in(path.parent().ok_or(io::ErrorKind::InvalidInput)?)
}

fn copy_xattrs(src: &File, dst: &File) -> io::Result<()> {
    // SAFETY:
    //   src and dst fds are valid
    //   passing null state is allowed
    //   flags are valid
    let rc = unsafe {
        libc::fcopyfile(
            src.as_raw_fd(),
            dst.as_raw_fd(),
            ptr::null_mut(),
            libc::COPYFILE_XATTR,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn copy_metadata(src: &File, dst: &File) -> io::Result<()> {
    // SAFETY:
    //   src and dst fds are valid
    //   passing null state is allowed
    //   flags are valid
    let rc = unsafe {
        libc::fcopyfile(
            src.as_raw_fd(),
            dst.as_raw_fd(),
            ptr::null_mut(),
            libc::COPYFILE_SECURITY,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
