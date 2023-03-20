use crate::decmpfs::CompressionType;
use crate::{compressor, decmpfs};
use std::io::{Seek, SeekFrom, Write};
use std::{io, mem};

pub trait Open {
    type ResourceFork: Write + Seek;

    fn open_resource_fork(self) -> io::Result<Self::ResourceFork>;
}

impl<W: Write + Seek, F: FnOnce() -> W> Open for F {
    type ResourceFork = W;

    fn open_resource_fork(self) -> io::Result<Self::ResourceFork> {
        Ok(self())
    }
}

enum WriterState<O: Open> {
    // Just used as a transition state, should never be there at the end of the write
    Empty,
    SingleBlock {
        // We still need to keep this openable, in case the block is too large to fit in an xattr
        open: O,
        block: Vec<u8>,
    },
    MultipleBlocks {
        block_sizes: Vec<u32>,
        resource_fork: O::ResourceFork,
    },
}

pub struct Writer<O: Open> {
    kind: compressor::Kind,
    uncompressed_size: u64,
    state: WriterState<O>,
}

impl<O: Open> Writer<O> {
    pub fn new(kind: compressor::Kind, uncompressed_size: u64, open: O) -> io::Result<Self> {
        let block_count = crate::num_blocks(uncompressed_size);
        let state = if block_count > 1 {
            let mut resource_fork = open.open_resource_fork()?;
            resource_fork.seek(SeekFrom::Start(kind.header_size(block_count)))?;

            WriterState::MultipleBlocks {
                block_sizes: Vec::with_capacity(block_count.try_into().unwrap()),
                resource_fork,
            }
        } else {
            WriterState::SingleBlock {
                open,
                block: Vec::new(),
            }
        };
        Ok(Self {
            kind,
            uncompressed_size,
            state,
        })
    }

    pub fn add_block(&mut self, new_block: Vec<u8>) -> io::Result<()> {
        assert!(new_block.len() as u64 <= u32::MAX as u64);

        match &mut self.state {
            WriterState::SingleBlock { block, .. } => {
                assert!(block.is_empty());
                *block = new_block;
                if block.len() > decmpfs::MAX_XATTR_DATA_SIZE {
                    self.force_move_to_resource_fork()?;
                }
            }
            WriterState::MultipleBlocks {
                block_sizes,
                resource_fork,
            } => {
                block_sizes.push(new_block.len() as u32);
                resource_fork.write_all(&new_block)?;
            }
            WriterState::Empty => unreachable!(),
        };
        Ok(())
    }

    pub fn finish_decmpfs_data(self, dst: &mut Vec<u8>) -> io::Result<()> {
        let mut extra_data = Vec::new();
        let storage = match self.state {
            WriterState::SingleBlock { block, .. } => {
                debug_assert!(!block.is_empty() || self.uncompressed_size == 0);
                extra_data = block;
                decmpfs::Storage::Xattr
            }
            WriterState::MultipleBlocks {
                block_sizes,
                resource_fork: data,
            } => {
                if block_sizes.len() as u64 != crate::num_blocks(self.uncompressed_size) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Wrong number of blocks",
                    ));
                }
                self.kind.finish(data, &block_sizes)?;
                decmpfs::Storage::ResourceFork
            }
            WriterState::Empty => unreachable!(),
        };

        let value = decmpfs::Value {
            compression_type: CompressionType::new(self.kind, storage),
            uncompressed_size: self.uncompressed_size,
            extra_data: &extra_data,
        };

        dst.reserve(value.len());
        value.write_to(dst)?;
        Ok(())
    }

    // Only called on single-block files, to convert to multiple blocks, even with a single block
    // because the block is too large to fit in an xattr
    fn force_move_to_resource_fork(&mut self) -> io::Result<()> {
        match mem::replace(&mut self.state, WriterState::Empty) {
            WriterState::SingleBlock { open, block } => {
                let mut resource_fork = open.open_resource_fork()?;
                resource_fork.seek(SeekFrom::Start(
                    self.kind
                        .header_size(crate::num_blocks(self.uncompressed_size)),
                ))?;
                resource_fork.write_all(&block)?;

                self.state = WriterState::MultipleBlocks {
                    block_sizes: vec![block.len() as u32],
                    resource_fork,
                };
            }
            _ => unreachable!(),
        };
        Ok(())
    }
}
