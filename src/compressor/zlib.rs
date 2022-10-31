use crate::decmpfs::{ZlibBlockInfo, ZLIB_BLOCK_TABLE_START, ZLIB_TRAILER};
use crate::try_read_all;
use flate2::bufread::{ZlibDecoder, ZlibEncoder};
use flate2::Compression;
use std::io::SeekFrom;
use std::{io, mem};

pub struct Zlib;

impl super::CompressorImpl for Zlib {
    fn blocks_start(block_count: u64) -> u64 {
        ZLIB_BLOCK_TABLE_START + mem::size_of::<u32>() as u64 + block_count * ZlibBlockInfo::SIZE
    }

    fn compress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize> {
        assert!(dst.len() > src.len());

        let encoder = ZlibEncoder::new(src, Compression::default());
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
        let decoder = ZlibDecoder::new(src);
        let bytes_read = try_read_all(decoder, dst)?;
        if bytes_read == dst.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }

        Ok(bytes_read)
    }

    fn finish<W: io::Write + io::Seek>(mut writer: W, block_sizes: &[u32]) -> io::Result<()> {
        let block_count =
            u32::try_from(block_sizes.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
        let compressed_data_size =
            u32::try_from(writer.stream_position()?).map_err(|_| io::ErrorKind::InvalidInput)?;
        writer.write_all(&ZLIB_TRAILER)?;

        writer.seek(SeekFrom::Start(0))?;
        writer.write_all(&u32::to_be_bytes(0x100))?;
        writer.write_all(&u32::to_be_bytes(compressed_data_size))?;
        writer.write_all(&u32::to_be_bytes(compressed_data_size - 0x100))?;
        writer.write_all(&u32::to_be_bytes(0x32))?;

        writer.seek(SeekFrom::Start(0x100))?;
        writer.write_all(&u32::to_be_bytes(compressed_data_size - 0x104))?;

        writer.write_all(&u32::to_le_bytes(block_count))?;
        let mut current_offset =
            u32::try_from(Self::blocks_start(block_count.into()) - ZLIB_BLOCK_TABLE_START).unwrap();
        for &size in block_sizes {
            let block_info = ZlibBlockInfo {
                offset: current_offset,
                compressed_size: size,
            };
            block_info.write_into(&mut writer)?;

            current_offset = current_offset.checked_add(size).ok_or_else(|| {
                io::Error::new(io::ErrorKind::Other, "offset too large for 32 bytes")
            })?;
        }
        debug_assert_eq!(
            writer.stream_position()?,
            Self::blocks_start(block_count.into())
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compressor::tests::compressor_round_trip;
    use crate::compressor::CompressorImpl;
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let mut compressor = Zlib;
        compressor_round_trip(&mut compressor);
    }

    #[test]
    fn finish() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        cursor.set_position(0x200);
        let block_sizes = &[10, 20, 30, 40, 10];

        let mut compressor = Zlib;
        compressor.finish(&mut cursor, block_sizes).unwrap();
        let result = cursor.into_inner();
        assert_eq!(
            result[..16],
            [0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0x32]
        );
        assert!(result[16..0x100].iter().all(|&b| b == 0));
        assert_eq!(result[0x100..0x104], u32::to_be_bytes(0x200 - 0x104));
        assert_eq!(
            result[0x104..0x108],
            u32::to_le_bytes(block_sizes.len() as _)
        );
    }
}
