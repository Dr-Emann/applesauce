use crate::compressor::Kind;
use crate::decmpfs::Storage;
use crate::{decmpfs, xattr, BLOCK_SIZE};
use resource_fork::ResourceFork;
use std::fs::File;
use std::io;
use std::io::{BufReader, Read, Seek, SeekFrom};

pub fn with_compressed_blocks<F, F2>(file: &File, f: F) -> io::Result<()>
where
    F: FnOnce(Kind) -> F2,
    F2: FnMut(&[u8]) -> io::Result<()>,
{
    let decmpfs_data = xattr::read(file, decmpfs::XATTR_NAME)?;
    let res = decmpfs::Value::from_data(decmpfs_data.as_ref().unwrap()).unwrap();
    let (kind, storage) = res.compression_type.compression_storage().unwrap();
    if !kind.supported() {
        return Err(todo!());
    }
    let mut per_block = f(kind);
    match storage {
        Storage::Xattr => per_block(res.extra_data)?,
        Storage::ResourceFork => {
            let mut rfork = BufReader::new(ResourceFork::new(file));
            let mut buf = Vec::with_capacity(BLOCK_SIZE);
            let block_info = kind.read_block_info(&mut rfork, res.uncompressed_size)?;
            for block in block_info {
                buf.clear();
                rfork.seek(SeekFrom::Start(block.offset.into()))?;
                rfork
                    .by_ref()
                    .take(block.compressed_size.into())
                    .read_to_end(&mut buf)?;
                per_block(&buf)?;
            }
        }
    }

    todo!()
}
