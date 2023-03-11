use std::io;
use std::io::Read;

pub mod compressor;
pub mod decmpfs;

pub const BLOCK_SIZE: usize = 0x10000;

#[must_use]
pub const fn num_blocks(size: u64) -> u64 {
    (size + (BLOCK_SIZE as u64 - 1)) / (BLOCK_SIZE as u64)
}

#[must_use]
pub const fn round_to_block_size(size: u64, block_size: u64) -> u64 {
    match size % block_size {
        0 => size,
        r => size + (block_size - r),
    }
}

fn try_read_all<R: Read>(mut r: R, buf: &mut [u8]) -> io::Result<usize> {
    let bulk_read_span = tracing::trace_span!(
        "try_read_all",
        len = buf.len(),
        read_len = tracing::field::Empty,
    );
    let full_len = buf.len();
    let mut remaining = buf;
    loop {
        let _enter = bulk_read_span.enter();
        let n = match r.read(remaining) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if n == 0 {
            break;
        }
        remaining = &mut remaining[n..];
        if remaining.is_empty() {
            return Ok(full_len);
        }
    }
    let read_len = full_len - remaining.len();

    bulk_read_span.record("read_len", read_len);
    Ok(read_len)
}
