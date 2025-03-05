use crate::xattr;
use applesauce_core::compressor::Kind;
use applesauce_core::decmpfs;
use applesauce_core::BLOCK_SIZE;
use resource_fork::ResourceFork;
use std::fs::File;
use std::io;

pub fn with_compressed_blocks<F, F2>(file: &File, f: F) -> io::Result<()>
where
    F: FnOnce(Kind) -> F2,
    F2: FnMut(&[u8]) -> io::Result<()>,
{
    let decmpfs_data = xattr::read(file, decmpfs::XATTR_NAME)?
        .ok_or_else(|| io::Error::other("file is not compressed"))?;
    let mut reader =
        applesauce_core::reader::Reader::new(&decmpfs_data, || ResourceFork::new(file))?;

    let mut per_block = f(reader.compression_kind());
    let mut buf = Vec::with_capacity(BLOCK_SIZE);
    loop {
        buf.clear();
        let has_block = reader.read_block_into(&mut buf)?;
        if !has_block {
            break;
        }
        per_block(&buf)?;
    }

    Ok(())
}
