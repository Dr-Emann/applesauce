use crate::compressor::CompressorImpl;
use crate::decmpfs;
use crate::decmpfs::BlockInfo;
use std::io::SeekFrom;
use std::marker::PhantomData;
use std::{io, mem};

pub trait Impl {
    const UNCOMPRESSED_PREFIX: Option<u8> = None;

    fn scratch_size() -> usize;

    unsafe fn encode(dst: &mut [u8], src: &[u8], scratch: &mut [u8]) -> usize;
    unsafe fn decode(dst: &mut [u8], src: &[u8], scratch: &mut [u8]) -> usize;
}

pub struct Lz<I> {
    buf: Box<[u8]>,
    _impl: PhantomData<I>,
}

impl<I: Impl> Lz<I> {
    pub fn new() -> Self {
        Self {
            buf: vec![0; I::scratch_size()].into_boxed_slice(),
            _impl: PhantomData,
        }
    }
}

impl<I: Impl> CompressorImpl for Lz<I> {
    fn blocks_start(block_count: u64) -> u64 {
        (block_count + 1) * mem::size_of::<u32>() as u64
    }

    fn compress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize> {
        assert!(dst.len() > src.len());

        let max_compress_size = if I::UNCOMPRESSED_PREFIX.is_some() {
            src.len()
        } else {
            dst.len()
        };
        // SAFETY:
        // dst is valid to write up to len bytes
        // len is either dst.len() or src.len(), and dst.len() > src.len()
        // src is initialised for len bytes
        // buf is valid to write up to scratch size bytes
        let len = unsafe { I::encode(&mut dst[..max_compress_size], src, &mut self.buf) };
        debug_assert!(len <= max_compress_size);
        if len == 0 {
            return if let Some(uncompressed_prefix) = I::UNCOMPRESSED_PREFIX {
                tracing::trace!("storing uncompressed data");
                dst[0] = uncompressed_prefix;
                dst[1..][..src.len()].copy_from_slice(src);
                Ok(src.len() + 1)
            } else {
                Err(io::ErrorKind::WriteZero.into())
            };
        }
        Ok(len)
    }

    fn decompress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize> {
        if src.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        // check if the data was stored uncompressed
        if let Some(uncompressed_prefix) = I::UNCOMPRESSED_PREFIX {
            if src[0] == uncompressed_prefix {
                let src = &src[1..];
                if dst.len() < src.len() {
                    return Err(io::ErrorKind::WriteZero.into());
                }
                dst[..src.len()].copy_from_slice(src);
                return Ok(src.len());
            }
        }
        // SAFETY:
        // dst is valid to write up to len bytes
        // src is initialised for len bytes
        // buf is valid to write up to scratch size bytes
        let len = unsafe { I::decode(dst, src, &mut self.buf) };
        debug_assert!(len < dst.len());
        if len == 0 || len == dst.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }
        Ok(len)
    }

    fn read_block_info<R: io::Read + io::Seek>(
        mut reader: R,
        orig_file_size: u64,
    ) -> io::Result<Vec<decmpfs::BlockInfo>> {
        reader.rewind()?;
        let block_count = crate::num_blocks(orig_file_size);

        let blocks_start = u32::try_from(Self::blocks_start(block_count)).unwrap();
        let mut result = Vec::with_capacity(
            block_count
                .try_into()
                .map_err(|_| io::ErrorKind::InvalidInput)?,
        );

        let mut buf = [0; mem::size_of::<u32>()];

        reader.read_exact(&mut buf)?;
        let mut last_offset = u32::from_le_bytes(buf);
        if last_offset != blocks_start {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "unexpected first block offset",
            ));
        }

        // LZ stores an offset before every block, and an extra for the end, we've
        //  read one offset, so we can read block_count more
        for _ in 0..block_count {
            reader.read_exact(&mut buf)?;
            let next_offset = u32::from_le_bytes(buf);
            let compressed_size = next_offset
                .checked_sub(last_offset)
                .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "compressed block overlap"))?;
            result.push(BlockInfo {
                offset: last_offset,
                compressed_size,
            });
            last_offset = next_offset;
        }

        // Check that the last offset is the end of the file
        let end_pos = reader.seek(SeekFrom::End(0))?;
        if end_pos != u64::from(last_offset) {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "last block does not end resource fork",
            ));
        }

        Ok(result)
    }

    fn finish<W: io::Write + io::Seek>(mut writer: W, block_sizes: &[u32]) -> io::Result<()> {
        let block_count = u32::try_from(block_sizes.len()).unwrap();
        let mut offset = u32::try_from(Self::blocks_start(block_count.into())).unwrap();

        writer.rewind()?;

        for &size in block_sizes {
            writer.write_all(&u32::to_le_bytes(offset))?;

            offset = offset.checked_add(size).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Other,
                    "Unable to represent offset in 32 bits",
                )
            })?;
        }
        // Write the final offset
        writer.write_all(&u32::to_le_bytes(offset))?;

        // This is logically a non-modifying operation, even if it takes &mut self, and can fail
        #[allow(clippy::debug_assert_with_mut_call)]
        {
            debug_assert_eq!(
                writer.stream_position()?,
                Self::blocks_start(block_count.into())
            );
        }
        Ok(())
    }
}
