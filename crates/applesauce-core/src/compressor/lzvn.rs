use crate::compressor::lz;
use libc::c_int;
use std::cmp;
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

pub type Lzvn = lz::Lz<Impl>;

pub enum Impl {}

// SAFETY: We return a consistent value for scratch_size, and rely on the impl to return a correct
//         value for the scratch size it will touch.
unsafe impl lz::Impl for Impl {
    const UNCOMPRESSED_PREFIX: Option<u8> = Some(0x06);

    fn scratch_size() -> usize {
        const {
            // lz implementation will align to a pointer, ensure that's enough for
            // the decoder state
            assert!(align_of::<DecoderState>() <= align_of::<*mut u8>());
        }
        lz::cached_size!(cmp::max(
            size_of::<DecoderState>(),
            // Safety: this function is always safe to call
            unsafe { lzvn_encode_scratch_size() },
        ))
    }

    unsafe fn encode(dst: &mut [u8], src: &[u8], scratch: NonNull<u8>) -> usize {
        // SAFETY: Buffers are valid for the specified lengths, and caller must ensure scratch is large enough
        let res = unsafe {
            lzvn_encode_buffer(
                dst.as_mut_ptr(),
                dst.len(),
                src.as_ptr(),
                src.len(),
                scratch.as_ptr().cast(),
            )
        };
        debug_assert!(res <= dst.len());
        res
    }

    #[no_mangle]
    unsafe fn decode(dst: &mut [u8], src: &[u8], scratch: NonNull<u8>) -> usize {
        const _: () = {
            assert!(align_of::<DecoderState>() <= align_of::<*mut u8>());
        };
        // SAFETY: caller will align `scratch` to a pointer, which we assert is enough above,
        //         and it must be at least `scratch_size`, which we ensure is at least as large as
        //         the size of `DecoderState`
        let state = unsafe { scratch.cast::<MaybeUninit<DecoderState>>().as_mut() };
        let state: &mut DecoderState = state.write(DecoderState::default());

        let src_range = src.as_ptr_range();
        state.src = src_range.start;
        state.src_end = src_range.end;

        let dst_range = dst.as_mut_ptr_range();
        state.dst = dst_range.start;
        state.dst_begin = dst_range.start;
        state.dst_end = dst_range.end;

        // SAFETY: state is fully initialized
        unsafe {
            lzvn_decode(state);
        }

        assert!(dst_range.contains(&state.dst));
        // SAFETY: lvzn_decode will have updated the dst ptr on state,
        //         but kept within range dst..dst_end
        unsafe { state.dst.offset_from_unsigned(dst.as_mut_ptr()) }
    }
}

// Forcing linking even if lzfse isn't included
const _: () = {
    let _ = lzfse_sys::lzfse_encode_buffer;
};

#[repr(C)]
#[derive(Default)]
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

#[test]
fn round_trip() {
    let mut compressor = Lzvn::new();
    super::tests::compressor_round_trip(&mut compressor);
}
