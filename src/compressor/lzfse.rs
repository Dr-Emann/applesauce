use crate::compressor::lz;
use lzfse_sys::{lzfse_decode_buffer, lzfse_decode_scratch_size, lzfse_encode_buffer};
use std::cmp;
use std::ffi::c_void;

pub type Lzfse = lz::Lz<Impl>;

pub enum Impl {}

impl lz::Impl for Impl {
    fn scratch_size() -> usize {
        // SAFETY: Both of these functions are always safe to call
        unsafe { cmp::max(lzfse_encode_scratch_size(), lzfse_decode_scratch_size()) }
    }

    unsafe fn encode(
        dst: *mut u8,
        dst_len: usize,
        src: *const u8,
        src_len: usize,
        scratch: *mut c_void,
    ) -> usize {
        debug_assert!(!dst.is_null());
        debug_assert!(!src.is_null());

        // No overlap
        debug_assert!(
            src as usize > (dst.add(dst_len) as usize)
                || (src.add(src_len) as usize) < dst as usize
        );
        let res = lzfse_encode_buffer(dst.cast(), dst_len, src.cast(), src_len, scratch);
        debug_assert!(res <= dst_len);
        res
    }

    unsafe fn decode(
        dst: *mut u8,
        dst_len: usize,
        src: *const u8,
        src_len: usize,
        scratch: *mut c_void,
    ) -> usize {
        debug_assert!(!dst.is_null());
        debug_assert!(!src.is_null());

        // No overlap
        debug_assert!(
            src as usize > (dst.add(dst_len) as usize) || src.add(src_len) as usize > dst as usize
        );
        let res = lzfse_decode_buffer(dst.cast(), dst_len, src.cast(), src_len, scratch);
        debug_assert!(res <= dst_len);
        res
    }
}

extern "C" {
    fn lzfse_encode_scratch_size() -> usize;
}

#[test]
fn round_trip() {
    let mut compressor = Lzfse::new();
    super::tests::compressor_round_trip(&mut compressor);
}
