use crate::compressor::CompressorImpl;
use crate::decmpfs::BlockInfo;
use std::alloc;
use std::io;
use std::io::SeekFrom;
use std::marker::PhantomData;
use std::ptr::NonNull;

macro_rules! cached_size {
    ($size:expr) => {{
        static CACHED_SIZE: ::std::sync::atomic::AtomicUsize =
            ::std::sync::atomic::AtomicUsize::new(0);
        let size = CACHED_SIZE.load(std::sync::atomic::Ordering::Relaxed);
        if size != 0 {
            size
        } else {
            let size = $size;
            debug_assert_ne!(size, 0);
            CACHED_SIZE.store(size, std::sync::atomic::Ordering::Relaxed);
            size
        }
    }};
}
pub(super) use cached_size;

/// An implementation of the LZ compression algorithm.
///
/// # Safety
///
/// Implementations of this trait must
/// - Always return the same size for `scratch_size()`
/// - Only access the up to `scratch_size()` bytes of the `scratch` buffer
pub unsafe trait Impl {
    const UNCOMPRESSED_PREFIX: Option<u8> = None;

    fn scratch_size() -> usize;

    unsafe fn encode(dst: &mut [u8], src: &[u8], scratch: NonNull<u8>) -> usize;
    unsafe fn decode(dst: &mut [u8], src: &[u8], scratch: NonNull<u8>) -> usize;
}

pub struct Lz<I: Impl> {
    buf: NonNull<u8>,
    _impl: PhantomData<I>,
}

// SAFETY: No interior mutability
unsafe impl<I: Impl> Send for Lz<I> {}
// SAFETY: No interior mutability
unsafe impl<I: Impl> Sync for Lz<I> {}

impl<I: Impl> Lz<I> {
    pub fn new() -> Self {
        let layout = Self::layout();
        assert!(layout.size() > 0);

        // SAFETY: layout is non-zero sized
        let buf = unsafe { alloc::alloc(layout) };
        let buf = NonNull::new(buf).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self {
            buf,
            _impl: PhantomData,
        }
    }

    fn layout() -> alloc::Layout {
        // Ensure at least one byte: not allowed to allocate a zero sized layout
        let size = I::scratch_size().max(1);
        alloc::Layout::from_size_align(size, align_of::<*mut u8>()).unwrap()
    }
}

impl<I: Impl> Drop for Lz<I> {
    fn drop(&mut self) {
        // SAFETY: `self.buf` was allocated with `alloc::alloc`, and `layout` must be constant
        //         because the implementor promised to return a constant scratch size
        unsafe {
            alloc::dealloc(self.buf.as_ptr(), Self::layout());
        }
    }
}

impl<I: Impl> CompressorImpl for Lz<I> {
    fn header_size(block_count: u64) -> u64 {
        (block_count + 1) * size_of::<u32>() as u64
    }

    fn compress(&mut self, dst: &mut [u8], src: &[u8], _level: u32) -> io::Result<usize> {
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
        let len = unsafe { I::encode(&mut dst[..max_compress_size], src, self.buf) };
        debug_assert!(len <= max_compress_size);

        if len == 0 {
            let uncompressed_prefix = I::UNCOMPRESSED_PREFIX.ok_or(io::ErrorKind::WriteZero)?;
            tracing::trace!("storing uncompressed data");
            dst[0] = uncompressed_prefix;
            dst[1..][..src.len()].copy_from_slice(src);
            Ok(src.len() + 1)
        } else {
            Ok(len)
        }
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
        let len = unsafe { I::decode(dst, src, self.buf) };
        debug_assert!(len < dst.len());
        if len == 0 || len == dst.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }
        Ok(len)
    }

    fn read_block_info<R: io::Read + io::Seek>(
        mut reader: R,
        orig_file_size: u64,
    ) -> io::Result<Vec<BlockInfo>> {
        reader.rewind()?;
        let block_count = crate::num_blocks(orig_file_size);

        let blocks_start = u32::try_from(Self::header_size(block_count)).unwrap();
        let mut result = Vec::with_capacity(
            block_count
                .try_into()
                .map_err(|_| io::ErrorKind::InvalidInput)?,
        );

        let mut buf = [0; size_of::<u32>()];

        reader.read_exact(&mut buf)?;
        let mut last_offset = u32::from_le_bytes(buf);
        if last_offset != blocks_start {
            return Err(io::Error::other("unexpected first block offset"));
        }

        // LZ stores an offset before every block, and an extra for the end, we've
        //  read one offset, so we can read block_count more
        for _ in 0..block_count {
            reader.read_exact(&mut buf)?;
            let next_offset = u32::from_le_bytes(buf);
            let compressed_size = next_offset
                .checked_sub(last_offset)
                .ok_or_else(|| io::Error::other("compressed block overlap"))?;
            result.push(BlockInfo {
                offset: last_offset,
                compressed_size,
            });
            last_offset = next_offset;
        }

        // Check that the last offset is the end of the file
        let end_pos = reader.seek(SeekFrom::End(0))?;
        if end_pos != u64::from(last_offset) {
            return Err(io::Error::other("last block does not end resource fork"));
        }

        Ok(result)
    }

    fn finish<W: io::Write + io::Seek>(mut writer: W, block_sizes: &[u32]) -> io::Result<()> {
        let block_count = u32::try_from(block_sizes.len()).unwrap();
        let mut offset = u32::try_from(Self::header_size(block_count.into())).unwrap();

        writer.rewind()?;

        for &size in block_sizes {
            writer.write_all(&u32::to_le_bytes(offset))?;

            offset = offset
                .checked_add(size)
                .ok_or_else(|| io::Error::other("Unable to represent offset in 32 bits"))?;
        }
        // Write the final offset
        writer.write_all(&u32::to_le_bytes(offset))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BLOCK_SIZE;
    use std::io::{Cursor, Write};

    struct FakeLzImpl;

    // SAFETY: We don't uphold any guarantees because we just panic everywhere
    unsafe impl Impl for FakeLzImpl {
        fn scratch_size() -> usize {
            unimplemented!()
        }

        unsafe fn encode(_: &mut [u8], _: &[u8], _: NonNull<u8>) -> usize {
            unimplemented!()
        }

        unsafe fn decode(_: &mut [u8], _: &[u8], _: NonNull<u8>) -> usize {
            unimplemented!()
        }
    }

    #[test]
    fn finish() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let block_sizes = &[10, 20, 30, 40, 10];
        let blocks_start = Lz::<FakeLzImpl>::header_size(block_sizes.len() as u64);
        let data_end = 110 + blocks_start as u32;
        cursor.set_position(data_end.into());
        // Ensure file is extended to size
        let _ = cursor.write(&[]).unwrap();

        Lz::<FakeLzImpl>::finish(&mut cursor, block_sizes).unwrap();
        let len = cursor.get_ref().len() as u64;
        assert_eq!(
            len,
            110 + Lz::<FakeLzImpl>::header_size(block_sizes.len() as u64)
                + Lz::<FakeLzImpl>::trailer_size()
        );

        cursor.set_position(0);
        let block_info =
            Lz::<FakeLzImpl>::read_block_info(&mut cursor, (block_sizes.len() * BLOCK_SIZE) as u64)
                .unwrap();
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
