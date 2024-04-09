use crate::decmpfs::{BlockInfo, ZLIB_BLOCK_TABLE_START, ZLIB_TRAILER};
use crate::try_read_all;
use flate2::bufread::{ZlibDecoder, ZlibEncoder};
use flate2::Compression;
use std::io::{Read, Seek, SeekFrom, Write};
use std::{io, mem};

pub struct Zlib;

impl super::CompressorImpl for Zlib {
    fn header_size(block_count: u64) -> u64 {
        ZLIB_BLOCK_TABLE_START + mem::size_of::<u32>() as u64 + block_count * BlockInfo::SIZE as u64
    }

    fn trailer_size() -> u64 {
        u64::try_from(ZLIB_TRAILER.len()).unwrap()
    }

    fn compress(&mut self, dst: &mut [u8], src: &[u8], level: u32) -> io::Result<usize> {
        assert!(dst.len() > src.len());

        let encoder = ZlibEncoder::new(src, Compression::new(level));
        let bytes_read = try_read_all(encoder, &mut dst[..src.len()])?;
        if bytes_read == src.len() {
            tracing::trace!("writing uncompressed data");
            dst[0] = 0xff;
            dst[1..][..src.len()].copy_from_slice(src);
            return Ok(src.len() + 1);
        }

        Ok(bytes_read)
    }

    fn decompress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize> {
        if src.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if src[0] == 0xff {
            let src = &src[1..];
            if dst.len() < src.len() {
                return Err(io::ErrorKind::WriteZero.into());
            }
            dst[..src.len()].copy_from_slice(src);
            return Ok(src.len());
        }
        let decoder = ZlibDecoder::new(src);
        let bytes_read = try_read_all(decoder, dst)?;
        if bytes_read == dst.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }

        Ok(bytes_read)
    }

    fn read_block_info<R: Read + Seek>(
        mut reader: R,
        orig_file_size: u64,
    ) -> io::Result<Vec<BlockInfo>> {
        let block_count = u32::try_from(crate::num_blocks(orig_file_size)).unwrap();

        let total_size = u32::try_from(reader.seek(SeekFrom::End(0))?).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "resource fork exceeds u32 range",
            )
        })?;
        let data_end = total_size - u32::try_from(ZLIB_TRAILER.len()).unwrap();

        reader.rewind()?;
        let mut header_buf = [0; HEADER_LEN];
        reader.read_exact(&mut header_buf)?;
        if header_buf != header(data_end) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "zlib header does not match expectation",
            ));
        }

        let mut buf = [0; 0x100 - HEADER_LEN];
        reader.read_exact(&mut buf)?;
        if buf.iter().any(|&b| b != 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected zeros between header and 0x100",
            ));
        }

        let mut buf = [0; mem::size_of::<u32>()];
        reader.read_exact(&mut buf)?;
        if buf != u32::to_be_bytes(data_end - 0x104) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected data at 0x100",
            ));
        }

        reader.read_exact(&mut buf)?;
        if buf != block_count.to_le_bytes() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "block count does not match computed value",
            ));
        }

        let mut result = Vec::with_capacity(block_count.try_into().unwrap());
        let mut buf = [0; BlockInfo::SIZE];
        for _ in 0..block_count {
            reader.read_exact(&mut buf)?;
            let mut block_info = BlockInfo::from_bytes(buf);
            block_info.offset += ZLIB_BLOCK_TABLE_START as u32;
            result.push(block_info);
        }

        reader.seek(SeekFrom::Start(data_end.into()))?;
        let mut trailer_buf = [0; ZLIB_TRAILER.len()];
        reader.read_exact(&mut trailer_buf)?;
        if trailer_buf != ZLIB_TRAILER {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "trailer does not match",
            ));
        }

        Ok(result)
    }

    fn finish<W: io::Write + io::Seek>(mut writer: W, block_sizes: &[u32]) -> io::Result<()> {
        let block_count =
            u32::try_from(block_sizes.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
        let data_end =
            u32::try_from(writer.stream_position()?).map_err(|_| io::ErrorKind::InvalidInput)?;
        writer.write_all(&ZLIB_TRAILER)?;

        // This is logically a non-modifying operation, even if it takes &mut self, and can fail
        #[allow(clippy::debug_assert_with_mut_call)]
        {
            debug_assert_eq!(
                writer.stream_position()?,
                u64::from(data_end) + Self::trailer_size()
            );
        }

        writer.rewind()?;
        writer.write_all(&header(data_end))?;

        writer.seek(SeekFrom::Start(0x100))?;
        writer.write_all(&u32::to_be_bytes(data_end - 0x104))?;

        writer.write_all(&u32::to_le_bytes(block_count))?;
        let mut current_offset =
            u32::try_from(Self::header_size(block_count.into()) - ZLIB_BLOCK_TABLE_START).unwrap();
        for &size in block_sizes {
            let block_info = BlockInfo {
                offset: current_offset,
                compressed_size: size,
            };
            writer.write_all(&block_info.to_bytes())?;

            current_offset = current_offset.checked_add(size).ok_or_else(|| {
                io::Error::new(io::ErrorKind::Other, "offset too large for 32 bytes")
            })?;
        }

        writer.flush()?;

        // This is logically a non-modifying operation, even if it takes &mut self, and can fail
        #[allow(clippy::debug_assert_with_mut_call)]
        {
            debug_assert_eq!(
                writer.stream_position()?,
                Self::header_size(block_count.into())
            );
        }
        Ok(())
    }
}

const HEADER_LEN: usize = 4 * mem::size_of::<u32>();
fn header(data_end: u32) -> [u8; HEADER_LEN] {
    let mut result = [0; HEADER_LEN];

    let mut writer = &mut result[..];
    writer.write_all(&u32::to_be_bytes(0x100)).unwrap();
    writer.write_all(&u32::to_be_bytes(data_end)).unwrap();
    writer
        .write_all(&u32::to_be_bytes(data_end - 0x100))
        .unwrap();
    writer.write_all(&u32::to_be_bytes(0x32)).unwrap();
    assert!(writer.is_empty());

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compressor::tests::compressor_round_trip;
    use crate::compressor::CompressorImpl;
    use crate::BLOCK_SIZE;
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let mut compressor = Zlib;
        compressor_round_trip(&mut compressor);
    }

    #[test]
    fn extra_size() {
        assert_eq!(Zlib::header_size(0) + Zlib::trailer_size(), 0x13A);
    }

    #[test]
    fn finish() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let block_sizes = &[10, 20, 30, 40, 10];
        let blocks_start = Zlib::header_size(block_sizes.len() as u64);
        let data_end = 110 + blocks_start as u32;
        cursor.set_position(data_end.into());
        // Ensure file is extended to size
        let _ = cursor.write(&[]).unwrap();

        Zlib::finish(&mut cursor, block_sizes).unwrap();
        let len = cursor.get_ref().len() as u64;
        assert_eq!(
            len,
            110 + Zlib::header_size(block_sizes.len() as u64) + Zlib::trailer_size()
        );

        let result = cursor.get_ref();
        assert_eq!(result[..16], header(data_end));
        assert!(result[16..0x100].iter().all(|&b| b == 0));
        assert_eq!(result[0x100..0x104], u32::to_be_bytes(data_end - 0x104));
        assert_eq!(
            result[0x104..0x108],
            u32::to_le_bytes(block_sizes.len() as _)
        );

        cursor.set_position(0);
        let block_info =
            Zlib::read_block_info(&mut cursor, (block_sizes.len() * BLOCK_SIZE) as u64).unwrap();
        let expected_block_info: Vec<BlockInfo> = block_sizes
            .iter()
            .scan(blocks_start as u32, |acc, &size| {
                let offset = *acc;
                *acc += size;
                Some(BlockInfo {
                    offset,
                    compressed_size: size,
                })
            })
            .collect();
        assert_eq!(block_info, expected_block_info);
    }
}
