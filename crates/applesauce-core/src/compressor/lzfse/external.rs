use crate::compressor::lz;
use lzfse_sys::{
    lzfse_decode_buffer, lzfse_decode_scratch_size, lzfse_encode_buffer, lzfse_encode_scratch_size,
};
use std::cmp;
use std::ptr::NonNull;

pub enum Impl {}

// SAFETY: We return a consistent value for scratch_size, and rely on the impl to return a correct
//         value for the scratch size it will touch.
unsafe impl lz::Impl for Impl {
    fn scratch_size() -> usize {
        // SAFETY: Both of these functions are always safe to call
        lz::cached_size!(unsafe {
            cmp::max(lzfse_encode_scratch_size(), lzfse_decode_scratch_size()).max(1)
        })
    }

    unsafe fn encode(dst: &mut [u8], src: &[u8], scratch: NonNull<u8>) -> usize {
        // SAFETY: Buffers are valid for the specified lengths, and caller must ensure scratch is large enough
        let res = unsafe {
            lzfse_encode_buffer(
                dst.as_mut_ptr().cast(),
                dst.len(),
                src.as_ptr().cast(),
                src.len(),
                scratch.as_ptr().cast(),
            )
        };
        debug_assert!(res <= dst.len());
        res
    }

    unsafe fn decode(dst: &mut [u8], src: &[u8], scratch: NonNull<u8>) -> usize {
        // SAFETY: Buffers are valid for the specified lengths, and caller must ensure scratch is large enough
        let res = unsafe {
            lzfse_decode_buffer(
                dst.as_mut_ptr().cast(),
                dst.len(),
                src.as_ptr().cast(),
                src.len(),
                scratch.as_ptr().cast(),
            )
        };
        debug_assert!(res <= dst.len());
        res
    }
}
