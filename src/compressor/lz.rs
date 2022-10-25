use crate::compressor::CompressorImpl;
use std::ffi::c_void;
use std::io::SeekFrom;
use std::marker::PhantomData;
use std::{io, mem};

pub trait Impl {
    const UNCOMPRESSED_PREFIX: Option<u8> = None;

    fn scratch_size() -> usize;

    unsafe fn encode(
        dst: *mut u8,
        dst_len: usize,
        src: *const u8,
        src_len: usize,
        scratch: *mut c_void,
    ) -> usize;
    unsafe fn decode(
        dst: *mut u8,
        dst_len: usize,
        src: *const u8,
        src_len: usize,
        scratch: *mut c_void,
    ) -> usize;
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
    fn blocks_start(&self, block_count: u64) -> u64 {
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
        let len = unsafe {
            I::encode(
                dst.as_mut_ptr(),
                max_compress_size,
                src.as_ptr(),
                src.len(),
                self.buf.as_mut_ptr().cast(),
            )
        };
        debug_assert!(len <= max_compress_size);
        if len == 0 {
            if let Some(uncompressed_prefix) = I::UNCOMPRESSED_PREFIX {
                dst[0] = uncompressed_prefix;
                dst[1..][..src.len()].copy_from_slice(src);
            } else {
                return Err(io::ErrorKind::WriteZero.into());
            }
        }
        Ok(len)
    }

    fn decompress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize> {
        // SAFETY:
        // dst is valid to write up to len bytes
        // src is initialised for len bytes
        // buf is valid to write up to scratch size bytes
        let len = unsafe {
            I::decode(
                dst.as_mut_ptr(),
                dst.len(),
                src.as_ptr(),
                src.len(),
                self.buf.as_mut_ptr().cast(),
            )
        };
        debug_assert!(len < dst.len());
        if len == 0 || len == dst.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }
        Ok(len)
    }

    fn finish<W: io::Write + io::Seek>(
        &mut self,
        mut writer: W,
        block_sizes: &[u32],
    ) -> io::Result<()> {
        let block_count = u32::try_from(block_sizes.len()).unwrap();
        let mut offset = u32::try_from(self.blocks_start(block_count.into())).unwrap();

        writer.seek(SeekFrom::Start(0))?;

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
        Ok(())
    }
}
