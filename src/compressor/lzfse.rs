use crate::compressor::lz;
use lzfse_sys::*;
use std::cmp;
use std::ffi::c_void;

pub type Lzfse = lz::Lz<LzfseImpl>;

pub enum LzfseImpl {}

impl lz::Impl for LzfseImpl {
    fn scratch_size() -> usize {
        unsafe { cmp::max(lzfse_encode_scratch_size(), lzfse_decode_scratch_size()) }
    }

    unsafe fn encode(
        dst: *mut u8,
        dst_len: usize,
        src: *const u8,
        src_len: usize,
        scratch: *mut c_void,
    ) -> usize {
        lzfse_encode_buffer(dst.cast(), dst_len, src.cast(), src_len, scratch)
    }

    unsafe fn decode(
        dst: *mut u8,
        dst_len: usize,
        src: *const u8,
        src_len: usize,
        scratch: *mut c_void,
    ) -> usize {
        lzfse_decode_buffer(dst.cast(), dst_len, src.cast(), src_len, scratch)
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