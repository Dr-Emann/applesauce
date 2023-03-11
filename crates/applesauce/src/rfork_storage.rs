use crate::xattr;
use applesauce_core::compressor::Kind;
use applesauce_core::decmpfs::{self, Storage};
use applesauce_core::BLOCK_SIZE;
use resource_fork::ResourceFork;
use std::fs::File;
use std::io;
use std::io::{BufReader, Read, Seek, SeekFrom};

pub fn with_compressed_blocks<F, F2>(file: &File, f: F) -> io::Result<()>
where
    F: FnOnce(Kind) -> F2,
    F2: FnMut(&[u8]) -> io::Result<()>,
{
    let decmpfs_data = xattr::read(file, decmpfs::XATTR_NAME)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "file is not compressed"))?;
    let res = decmpfs::Value::from_data(&decmpfs_data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let (kind, storage) = res
        .compression_type
        .compression_storage()
        .filter(|(kind, _)| kind.supported())
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "unsupported compression kind"))?;

    let mut per_block = f(kind);
    match storage {
        Storage::Xattr => per_block(res.extra_data)?,
        Storage::ResourceFork => {
            let mut rfork = BufReader::new(ResourceFork::new(file));
            let mut buf = Vec::with_capacity(BLOCK_SIZE);
            let block_info = kind.read_block_info(&mut rfork, res.uncompressed_size)?;
            let mut last_offset: Option<u32> = None;
            for block in block_info {
                buf.clear();
                match last_offset {
                    Some(o) => {
                        let diff = i64::from(block.offset) - i64::from(o);
                        rfork.seek_relative(diff)?;
                    }
                    None => {
                        rfork.seek(SeekFrom::Start(block.offset.into()))?;
                    }
                }
                let bytes_read = rfork
                    .by_ref()
                    .take(block.compressed_size.into())
                    .read_to_end(&mut buf)?;
                if bytes_read < block.compressed_size as usize {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }

                last_offset = Some(
                    block
                        .offset
                        .checked_add(block.compressed_size)
                        .ok_or(io::ErrorKind::InvalidData)?,
                );
                per_block(&buf)?;
            }
        }
    }

    Ok(())
}
