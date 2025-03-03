pub mod compressor;
pub mod decmpfs;
pub mod reader;
pub mod writer;

pub const BLOCK_SIZE: usize = 0x10000;

/// Returns the number of blocks needed to store `size` bytes.
#[must_use]
#[inline]
pub const fn num_blocks(size: u64) -> u64 {
    size.div_ceil(BLOCK_SIZE as u64)
}

/// Rounds `size` up to the nearest multiple of `block_size`.
///
/// If `size` is already a multiple of `block_size`, it is returned unchanged.
#[must_use]
#[inline]
pub const fn round_to_block_size(size: u64, block_size: u64) -> u64 {
    match size % block_size {
        0 => size,
        r => size + (block_size - r),
    }
}
