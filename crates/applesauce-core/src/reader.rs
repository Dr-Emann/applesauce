use crate::decmpfs::{BlockInfo, Storage};
use crate::{compressor, decmpfs};
use std::io::{self, BufReader, Cursor, Read, Seek};

pub trait Open {
    type ResourceFork: Read + Seek;

    fn open_resource_fork(self) -> io::Result<Self::ResourceFork>;
}

impl<R: Read + Seek, F: FnOnce() -> R> Open for F {
    type ResourceFork = R;

    fn open_resource_fork(self) -> io::Result<Self::ResourceFork> {
        Ok(self())
    }
}

enum State<R> {
    Xattr(Cursor<Vec<u8>>),
    ResourceFork {
        // Stored in reverse order, so that we can pop() them off
        block_infos: Vec<BlockInfo>,
        last_offset: u32,
        reader: BufReader<R>,
    },
}

pub struct Reader<R> {
    kind: compressor::Kind,
    state: State<R>,
}

impl<R: Read + Seek> Reader<R> {
    pub fn new<O>(decmpfs_data: Vec<u8>, open: O) -> io::Result<Self>
    where
        O: Open<ResourceFork = R>,
    {
        let decmpfs_value = decmpfs::Value::from_data(&decmpfs_data)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let (kind, storage) = decmpfs_value
            .compression_type
            .compression_storage()
            .filter(|(kind, _)| kind.supported())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Other,
                    "unsupported compression kind or storage",
                )
            })?;
        let state = match storage {
            Storage::Xattr => {
                let mut cursor = Cursor::new(decmpfs_data);
                cursor.set_position(decmpfs::HEADER_LEN as u64);
                State::Xattr(cursor)
            }
            Storage::ResourceFork => {
                let mut rfork = BufReader::new(open.open_resource_fork()?);
                let mut blocks_info =
                    kind.read_block_info(&mut rfork, decmpfs_value.uncompressed_size)?;

                // Seek back to the beginning of the resource fork
                rfork.rewind()?;
                // Reverse the block infos so that we can pop() them off
                blocks_info.reverse();

                State::ResourceFork {
                    block_infos: blocks_info,
                    last_offset: 0,
                    reader: rfork,
                }
            }
        };
        Ok(Self { kind, state })
    }

    pub fn read_block_into(&mut self, dst: &mut Vec<u8>) -> io::Result<bool> {
        match &mut self.state {
            State::Xattr(cursor) => cursor.read_to_end(dst).map(|bytes_read| bytes_read > 0),
            State::ResourceFork {
                block_infos,
                last_offset,
                reader,
            } => {
                let block = match block_infos.pop() {
                    Some(block) => block,
                    None => return Ok(false),
                };
                let diff = i64::from(block.offset) - i64::from(*last_offset);
                reader.seek_relative(diff)?;

                let bytes_read = reader
                    .by_ref()
                    .take(block.compressed_size.into())
                    .read_to_end(dst)?;
                if bytes_read < block.compressed_size as usize {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                *last_offset = block
                    .offset
                    .checked_add(block.compressed_size)
                    .ok_or(io::ErrorKind::InvalidData)?;
                Ok(true)
            }
        }
    }

    pub fn compression_kind(&self) -> compressor::Kind {
        self.kind
    }

    pub fn remaining_blocks(&self) -> usize {
        match &self.state {
            State::Xattr(cursor) => {
                let remaining = cursor.get_ref().len() as u64 - cursor.position();
                usize::from(remaining > 0)
            }
            State::ResourceFork { block_infos, .. } => block_infos.len(),
        }
    }
}
