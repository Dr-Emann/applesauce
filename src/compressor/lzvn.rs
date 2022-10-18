use crate::compressor::lz;
use libc::c_int;
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::{cmp, mem, slice};

pub type Lzvn = lz::Lz<LzvnImpl>;

pub enum LzvnImpl {}

impl lz::Impl for LzvnImpl {
    fn scratch_size() -> usize {
        cmp::max(mem::size_of::<DecoderState>(), unsafe {
            lzvn_encode_scratch_size()
        })
    }

    unsafe fn encode(
        dst: *mut u8,
        dst_len: usize,
        src: *const u8,
        src_len: usize,
        scratch: *mut c_void,
    ) -> usize {
        lzvn_encode_buffer(dst, dst_len, src, src_len, scratch)
    }

    unsafe fn decode(
        dst: *mut u8,
        dst_len: usize,
        src: *const u8,
        src_len: usize,
        scratch: *mut c_void,
    ) -> usize {
        decode(
            slice::from_raw_parts_mut(dst, dst_len),
            slice::from_raw_parts(src, src_len),
            &mut *scratch.cast::<MaybeUninit<_>>(),
        )
    }
}

// Forcing linking even if lzfse isn't included
const _: () = {
    let _ = lzfse_sys::lzfse_encode_buffer;
};

#[repr(C)]
struct DecoderState {
    src: *const u8,
    src_end: *const u8,

    dst: *mut u8,
    dst_begin: *mut u8,
    dst_end: *mut u8,
    dst_current: *mut u8,

    l: usize,
    m: usize,
    d: usize,

    d_prev: isize,

    end_of_stream: c_int,
}

// These symbols are actually provided by lzfse, as long as we're linking to lzfse,
// these will be available
extern "C" {
    fn lzvn_encode_scratch_size() -> usize;

    fn lzvn_encode_buffer(
        dst: *mut u8,
        dst_size: usize,
        src: *const u8,
        src_size: usize,
        work: *mut c_void,
    ) -> usize;

    fn lzvn_decode(state: *mut DecoderState);
}

fn decode(dst: &mut [u8], src: &[u8], buf: &mut MaybeUninit<DecoderState>) -> usize {
    unsafe {
        buf.as_mut_ptr().write_bytes(0, 1);
        let state = buf.assume_init_mut();
        state.src = src.as_ptr();
        state.src_end = src.as_ptr().add(src.len());
        state.dst = dst.as_mut_ptr();
        state.dst_begin = dst.as_mut_ptr();
        state.dst_end = dst.as_mut_ptr().add(dst.len());
        lzvn_decode(state);
        state.dst.offset_from(dst.as_mut_ptr()) as usize
    }
}

#[test]
fn round_trip() {
    let mut compressor = Lzvn::new();
    super::tests::compressor_round_trip(&mut compressor);
}
