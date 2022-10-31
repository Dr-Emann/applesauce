use crate::decmpfs::CompressionType;
use crate::resource_fork::ResourceFork;
use crate::theads::ThreadJoiner;
use crate::{
    compressor, decmpfs, num_blocks, remove_xattr, reset_times, resource_fork, seq_queue,
    set_flags, set_xattr,
};
use std::fs::{File, Metadata};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::os::macos::fs::MetadataExt;
use std::sync::Arc;
use std::{io, thread};

pub struct WorkItem {
    pub file: Arc<File>,
    pub blocks: seq_queue::Receiver<io::Result<Vec<u8>>>,
    pub metadata: Metadata,
}

pub struct WriterThreads {
    // Order is important: Drop happens top to bottom, drop the sender before trying to join the threads
    tx: crossbeam_channel::Sender<WorkItem>,
    joiner: ThreadJoiner,
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
            joiner: ThreadJoiner::new(threads),
        }
    }

    pub fn submit(&self, item: WorkItem) -> Result<(), crossbeam_channel::SendError<WorkItem>> {
        self.tx.send(item)
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

    fn write_blocks(
        &mut self,
        resource_fork: &mut ResourceFork<'_>,
        blocks: &seq_queue::Receiver<io::Result<Vec<u8>>>,
    ) -> io::Result<WriteDest> {
        let block1 = match blocks.recv() {
            Ok(block) => block?,
            Err(_) => return Ok(WriteDest::Xattr { data: Vec::new() }),
        };
        let block2 = match blocks.recv() {
            Ok(block) => block?,
            Err(_) => {
                // no second block
                if block1.len() <= decmpfs::MAX_XATTR_DATA_SIZE {
                    tracing::debug!("compressible inside xattr");
                    return Ok(WriteDest::Xattr { data: block1 });
                } else {
                    self.block_sizes.push(block1.len().try_into().unwrap());
                    resource_fork.write_all(&block1)?;
                    return Ok(WriteDest::BlocksWritten);
                }
            }
        };
        self.block_sizes.push(block1.len().try_into().unwrap());
        resource_fork.write_all(&block1)?;
        self.block_sizes.push(block2.len().try_into().unwrap());
        resource_fork.write_all(&block2)?;
        drop((block1, block2));

        for block in blocks {
            let block = block?;

            self.block_sizes.push(block.len().try_into().unwrap());
            resource_fork.write_all(&block)?;
        }
        Ok(WriteDest::BlocksWritten)
    }

    fn handle_work_item(&mut self, item: WorkItem) {
        let file_size = item.metadata.len();
        let block_count: u32 = num_blocks(file_size).try_into().unwrap();

        self.block_sizes.clear();
        self.block_sizes.reserve(block_count.try_into().unwrap());

        let mut resource_fork = ResourceFork::new(&item.file);
        resource_fork
            .seek(SeekFrom::Start(
                self.compressor_kind.blocks_start(block_count.into()),
            ))
            .unwrap();

        let res = self
            .write_blocks(&mut resource_fork, &item.blocks)
            .and_then(|write_dest| {
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
                header.write_into(&mut self.decomp_xattr_val_buf)?;

                if storage == decmpfs::Storage::ResourceFork {
                    let mut resource_fork = BufWriter::new(resource_fork);
                    self.compressor_kind
                        .finish(&mut resource_fork, &self.block_sizes)?;
                    resource_fork.flush()?;
                } else {
                    self.decomp_xattr_val_buf.extend_from_slice(&extra_data);
                }
                set_xattr(
                    &item.file,
                    decmpfs::XATTR_NAME,
                    &self.decomp_xattr_val_buf,
                    0,
                )?;
                Ok(())
            })
            .and_then(|()| {
                // TODO: Decompress back into file on error
                item.file.set_len(0)
            })
            .and_then(|()| set_flags(&item.file, item.metadata.st_flags() | libc::UF_COMPRESSED));

        if let Err(e) = res {
            tracing::error!("error while writing compressed file: {}", e);
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
        if let Err(e) = item.file.sync_all() {
            tracing::error!("error syncing file: {}", e);
        }
    }
}

fn thread_impl(compressor_kind: compressor::Kind, rx: crossbeam_channel::Receiver<WorkItem>) {
    let mut writer = Writer::new(compressor_kind);
    for item in rx {
        writer.handle_work_item(item);
    }
}