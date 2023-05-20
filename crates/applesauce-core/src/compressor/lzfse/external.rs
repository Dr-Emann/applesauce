use crate::compressor::lz;
use lzfse_sys::{lzfse_decode_buffer, lzfse_decode_scratch_size, lzfse_encode_buffer};
use std::cmp;

pub enum Impl {}

impl lz::Impl for Impl {
    fn scratch_size() -> usize {
        // SAFETY: Both of these functions are always safe to call
        unsafe { cmp::max(lzfse_encode_scratch_size(), lzfse_decode_scratch_size()) }
    }

    unsafe fn encode(dst: &mut [u8], src: &[u8], scratch: &mut [u8]) -> usize {
        // SAFETY: function is always safe to call
        debug_assert!(scratch.len() >= unsafe { lzfse_encode_scratch_size() });

        // SAFETY: Buffers are valid for the specified lengths, and caller must ensure scratch is large enough
        let res = unsafe {
            lzfse_encode_buffer(
                dst.as_mut_ptr().cast(),
                dst.len(),
                src.as_ptr().cast(),
                src.len(),
                scratch.as_mut_ptr().cast(),
            )
        };
        debug_assert!(res <= dst.len());
        res
    }

    unsafe fn decode(dst: &mut [u8], src: &[u8], scratch: &mut [u8]) -> usize {
        // SAFETY: function is always safe to call
        debug_assert!(scratch.len() >= unsafe { lzfse_decode_scratch_size() });

        // SAFETY: Buffers are valid for the specified lengths, and caller must ensure scratch is large enough
        let res = unsafe {
            lzfse_decode_buffer(
                dst.as_mut_ptr().cast(),
                dst.len(),
                src.as_ptr().cast(),
                src.len(),
                scratch.as_mut_ptr().cast(),
            )
        };
        debug_assert!(res <= dst.len());
        res
    }
}

extern "C" {
    fn lzfse_encode_scratch_size() -> usize;
}
