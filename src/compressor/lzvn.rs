use crate::compressor::lz;
use libc::c_int;
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::{cmp, mem, slice};

pub type Lzvn = lz::Lz<Impl>;

pub enum Impl {}

impl lz::Impl for Impl {
    const UNCOMPRESSED_PREFIX: Option<u8> = Some(0x06);

    fn scratch_size() -> usize {
        cmp::max(
            mem::size_of::<DecoderState>() + mem::align_of::<DecoderState>(),
            // Safety: this function is always safe to call
            unsafe { lzvn_encode_scratch_size() },
        )
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
        debug_assert!(!scratch.is_null());

        // No overlap
        debug_assert!(
            src as usize >= (dst.add(dst_len) as usize)
                || (src.add(src_len) as usize) <= dst as usize
        );
        let res = lzvn_encode_buffer(dst, dst_len, src, src_len, scratch);
        debug_assert!(res <= dst_len);
        res
    }

    unsafe fn decode(
        dst: *mut u8,
        dst_len: usize,
        src: *const u8,
        src_len: usize,
        _scratch: *mut c_void,
    ) -> usize {
        debug_assert!(!dst.is_null());
        debug_assert!(!src.is_null());

        // No overlap
        debug_assert!(
            src as usize >= (dst.add(dst_len) as usize)
                || (src.add(src_len) as usize) <= dst as usize
        );
        decode(
            slice::from_raw_parts_mut(dst, dst_len),
            slice::from_raw_parts(src, src_len),
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

fn decode(dst: &mut [u8], src: &[u8]) -> usize {
    // SAFETY: decoder state is all numeric and safe to zero init
    let mut state: DecoderState = unsafe { MaybeUninit::zeroed().assume_init() };

    let src_range = src.as_ptr_range();
    state.src = src_range.start;
    state.src_end = src_range.end;

    let dst_range = dst.as_mut_ptr_range();
    state.dst = dst_range.start;
    state.dst_begin = dst_range.start;
    state.dst_end = dst_range.end;

    // SAFETY: state is fully initialized
    unsafe {
        lzvn_decode(&mut state);
    }
    // SAFETY: lvzn_decode will have updated the dst ptr on state,
    //         but kept within range dst..dst_end
    unsafe { state.dst.offset_from(dst.as_mut_ptr()) as usize }
}

#[test]
fn round_trip() {
    let mut compressor = Lzvn::new();
    super::tests::compressor_round_trip(&mut compressor);
}
