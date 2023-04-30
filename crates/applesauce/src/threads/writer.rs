use crate::threads::{BgWork, Context, Mode, WorkHandler};
use crate::{reset_times, seq_queue, set_flags, xattr};
use applesauce_core::compressor::Kind;
use applesauce_core::decmpfs;
use resource_fork::ResourceFork;
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, BufWriter, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::macos::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::{cmp, io, ptr};
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

    fn queue_capacity(&self) -> usize {
        4
    }
}

pub(super) struct Handler {
    decomp_xattr_val_buf: Vec<u8>,
}

impl Handler {
    fn new() -> Self {
        Self {
            decomp_xattr_val_buf: Vec::with_capacity(decmpfs::MAX_XATTR_SIZE),
        }
    }

    #[tracing::instrument(level = "debug", skip_all, err)]
    fn write_blocks(
        &mut self,
        context: &Context,
        writer: &mut applesauce_core::writer::Writer<impl applesauce_core::writer::Open>,
        chunks: seq_queue::Receiver<io::Result<Chunk>>,
    ) -> io::Result<()> {
        let block_span = tracing::debug_span!("write block");

        let mut total_compressed_size = 0;
        let minimum_compression_ratio = match context.operation.mode {
            Mode::Compress {
                minimum_compression_ratio,
                ..
            } => minimum_compression_ratio,
            _ => unreachable!("write_blocks called in non-compress mode"),
        };
        let max_compressed_size = (context.orig_size as f64 * minimum_compression_ratio) as u64;

        let chunks = crate::instrumented_iter(chunks, tracing::debug_span!("waiting for chunk"));
        for chunk in chunks {
            let chunk = chunk?;

            total_compressed_size += u64::try_from(chunk.block.len()).unwrap();
            if total_compressed_size > max_compressed_size {
                context.progress.not_compressible_enough(&context.path);
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "did not compress to at least {}% of original size",
                        minimum_compression_ratio * 100.0
                    ),
                ));
            }

            let Chunk { block, orig_size } = chunk;
            let _enter = block_span.enter();

            writer.add_block(&block)?;
            context.progress.increment(orig_size);
        }
        Ok(())
    }

    fn write_compressed_file(
        &mut self,
        mut item: WorkItem,
        compressor_kind: Kind,
    ) -> io::Result<()> {
        let uncompressed_file_size = item.metadata.len();

        let mut tmp_file = tmp_file_for(&item.context.path)?;
        copy_xattrs(&item.file, tmp_file.as_file())?;

        let mut writer =
            applesauce_core::writer::Writer::new(compressor_kind, uncompressed_file_size, || {
                BufWriter::new(ResourceFork::new(tmp_file.as_file()))
            })?;

        self.write_blocks(&item.context, &mut writer, item.blocks)?;

        self.decomp_xattr_val_buf.clear();
        writer.finish_decmpfs_data(&mut self.decomp_xattr_val_buf)?;
        {
            let _entered = tracing::debug_span!("set decmpfs xattr").entered();
            xattr::set(
                tmp_file.as_file(),
                decmpfs::XATTR_NAME,
                &self.decomp_xattr_val_buf,
                0,
            )?;
        }

        copy_metadata(&item.file, tmp_file.as_file())?;
        set_flags(
            tmp_file.as_file(),
            item.metadata.st_flags() | libc::UF_COMPRESSED,
        )?;

        if item.context.verify {
            let _entered = tracing::info_span!("verify").entered();

            let orig_file = Arc::get_mut(&mut item.file)
                .expect("Reader should drop file before finishing writing blocks, writer should have the only reference");
            let mut orig_file = BufReader::new(orig_file);
            let mut new_file = BufReader::new(tmp_file.as_file_mut());

            orig_file.rewind()?;
            new_file.rewind()?;

            ensure_identical_files(orig_file, new_file).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "verification failed: {e}, {} unchanged",
                        item.context.path.display()
                    ),
                )
            })?;
        }

        let new_file = {
            let _entered = tracing::debug_span!("rename tmp file").entered();
            tmp_file.persist(&item.context.path)?
        };
        if let Err(e) = reset_times(&new_file, &item.metadata) {
            tracing::error!("Unable to reset times: {e}");
        }
        Ok(())
    }

    fn write_uncompressed_file(&mut self, item: WorkItem) -> io::Result<()> {
        let mut tmp_file = tmp_file_for(&item.context.path)?;
        copy_xattrs(&item.file, tmp_file.as_file())?;

        let chunks =
            crate::instrumented_iter(item.blocks, tracing::debug_span!("waiting for chunk"));
        for chunk in chunks {
            let chunk = chunk?;
            tmp_file.write_all(&chunk.block)?;
            // Increment progress by the uncompressed size of the block,
            // not the "original" (compressed) size
            item.context.progress.increment(chunk.block.len() as u64);
        }

        copy_metadata(&item.file, tmp_file.as_file())?;
        set_flags(
            tmp_file.as_file(),
            item.metadata.st_flags() & !libc::UF_COMPRESSED,
        )?;

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

        let res = match context.operation.mode {
            Mode::Compress { kind, .. } => self.write_compressed_file(item, kind),
            Mode::DecompressManually | Mode::DecompressByReading => {
                self.write_uncompressed_file(item)
            }
        };

        if res.is_ok() {
            let compressing = context.operation.mode.is_compressing();
            let prefix = if compressing { "" } else { "de" };
            tracing::info!("Successfully {prefix}compressed {}", context.path.display());
        }
    }
}

#[tracing::instrument(level="debug", skip_all, err, fields(path=%path.display()))]
fn tmp_file_for(path: &Path) -> io::Result<NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    if let Some(name) = path.file_name() {
        builder.prefix(name);
    }
    builder.tempfile_in(path.parent().ok_or(io::ErrorKind::InvalidInput)?)
}

#[tracing::instrument(level = "debug", skip_all, err)]
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

#[tracing::instrument(level = "debug", skip_all, err)]
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

fn ensure_identical_files<R1: BufRead, R2: BufRead>(mut lhs: R1, mut rhs: R2) -> io::Result<()> {
    loop {
        let l = lhs.fill_buf()?;
        let r = rhs.fill_buf()?;

        if l.is_empty() && r.is_empty() {
            return Ok(());
        }
        if l.is_empty() || r.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Files are not the same size",
            ));
        }

        let min_len = cmp::min(l.len(), r.len());
        let l = &l[..min_len];
        let r = &r[..min_len];

        if l != r {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Files are not identical",
            ));
        }

        lhs.consume(min_len);
        rhs.consume(min_len)
    }
}
