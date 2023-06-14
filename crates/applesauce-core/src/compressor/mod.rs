#[cfg(feature = "lzvn")]
use self::lzvn::Lzvn;
// Enable if feature lzfse or system-lzfse is enabled:
#[cfg(any(feature = "lzfse", feature = "system-lzfse"))]
use self::lzfse::Lzfse;
#[cfg(feature = "zlib")]
use self::zlib::Zlib;
use crate::decmpfs;
use crate::decmpfs::BlockInfo;
use std::{fmt, io};

#[cfg(any(feature = "lzfse", feature = "lzvn"))]
mod lz;
#[cfg(feature = "lzfse")]
mod lzfse;
#[cfg(feature = "lzvn")]
mod lzvn;
#[cfg(feature = "zlib")]
mod zlib;

pub(crate) trait CompressorImpl {
    /// The offset to start data at, for the specified number of blocks
    #[must_use]

    fn header_size(block_count: u64) -> u64;

    #[must_use]
    fn trailer_size() -> u64 {
        0
    }

    /// The extra size required to store `block_count` blocks, other than the data itself
    ///
    /// This defaults to `blocks_start`, but can be overridden if the compressor requires more space
    /// after the data as well
    #[must_use]
    fn extra_size(block_count: u64) -> u64 {
        Self::header_size(block_count) + Self::trailer_size()
    }

    fn compress(&mut self, dst: &mut [u8], src: &[u8], level: u32) -> io::Result<usize>;
    fn decompress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize>;

    fn read_block_info<R: io::Read + io::Seek>(
        reader: R,
        orig_file_size: u64,
    ) -> io::Result<Vec<decmpfs::BlockInfo>>;

    fn finish<W: io::Write + io::Seek>(writer: W, block_sizes: &[u32]) -> io::Result<()>;
}

pub struct Compressor(Data);

impl Compressor {
    #[cfg(feature = "zlib")]
    #[must_use]
    pub fn zlib() -> Self {
        Self(Data::Zlib(Zlib))
    }

    #[cfg(feature = "lzfse")]
    #[must_use]
    pub fn lzfse() -> Self {
        Self(Data::Lzfse(Lzfse::new()))
    }

    #[cfg(feature = "lzvn")]
    #[must_use]
    pub fn lzvn() -> Self {
        Self(Data::Lzvn(Lzvn::new()))
    }

    #[must_use]
    pub fn kind(&self) -> Kind {
        match self.0 {
            #[cfg(feature = "zlib")]
            Data::Zlib(_) => Kind::Zlib,
            #[cfg(feature = "lzfse")]
            Data::Lzfse(_) => Kind::Lzfse,
            #[cfg(feature = "lzvn")]
            Data::Lzvn(_) => Kind::Lzvn,
        }
    }
}

enum Data {
    #[cfg(feature = "zlib")]
    Zlib(Zlib),
    #[cfg(feature = "lzfse")]
    Lzfse(Lzfse),
    #[cfg(feature = "lzvn")]
    Lzvn(Lzvn),
}

impl Compressor {
    #[must_use]
    pub fn blocks_start(&self, block_count: u64) -> u64 {
        self.kind().header_size(block_count)
    }

    pub fn compress(&mut self, dst: &mut [u8], src: &[u8], level: u32) -> io::Result<usize> {
        match self.0 {
            #[cfg(feature = "zlib")]
            Data::Zlib(ref mut i) => i.compress(dst, src, level),
            #[cfg(feature = "lzfse")]
            Data::Lzfse(ref mut i) => i.compress(dst, src, level),
            #[cfg(feature = "lzvn")]
            Data::Lzvn(ref mut i) => i.compress(dst, src, level),
        }
    }

    pub fn decompress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize> {
        match self.0 {
            #[cfg(feature = "zlib")]
            Data::Zlib(ref mut i) => i.decompress(dst, src),
            #[cfg(feature = "lzfse")]
            Data::Lzfse(ref mut i) => i.decompress(dst, src),
            #[cfg(feature = "lzvn")]
            Data::Lzvn(ref mut i) => i.decompress(dst, src),
        }
    }

    pub fn finish<W: io::Write + io::Seek>(
        &mut self,
        writer: W,
        block_sizes: &[u32],
    ) -> io::Result<()> {
        self.kind().finish(writer, block_sizes)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Zlib = 0,
    Lzvn,
    Lzfse,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Kind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Kind::Zlib => "ZLIB",
            Kind::Lzvn => "LZVN",
            Kind::Lzfse => "LZFSE",
        }
    }

    #[must_use]
    #[inline]
    pub const fn supported(self) -> bool {
        // Clippy falsely sees these arms as identical:
        //   https://github.com/rust-lang/rust-clippy/issues/9775
        #[allow(clippy::match_same_arms)]
        match self {
            Kind::Zlib => cfg!(feature = "zlib"),
            Kind::Lzvn => cfg!(feature = "lzvn"),
            Kind::Lzfse => cfg!(feature = "lzfse"),
        }
    }

    #[must_use]
    pub fn compressor(self) -> Option<Compressor> {
        let data = match self {
            #[cfg(feature = "zlib")]
            Kind::Zlib => Data::Zlib(Zlib),
            #[cfg(feature = "lzfse")]
            Kind::Lzfse => Data::Lzfse(Lzfse::new()),
            #[cfg(feature = "lzvn")]
            Kind::Lzvn => Data::Lzvn(Lzvn::new()),
            #[allow(unreachable_patterns)]
            _ => return None,
        };
        Some(Compressor(data))
    }

    #[must_use]
    pub fn header_size(self, block_count: u64) -> u64 {
        match self {
            #[cfg(feature = "zlib")]
            Kind::Zlib => Zlib::header_size(block_count),
            #[cfg(feature = "lzvn")]
            Kind::Lzvn => Lzvn::header_size(block_count),
            #[cfg(feature = "lzfse")]
            Kind::Lzfse => Lzfse::header_size(block_count),
            #[allow(unreachable_patterns)]
            _ => panic!("Unsupported compression kind {self}"),
        }
    }

    pub fn read_block_info<R: io::Read + io::Seek>(
        self,
        reader: R,
        orig_file_size: u64,
    ) -> io::Result<Vec<BlockInfo>> {
        match self {
            #[cfg(feature = "zlib")]
            Kind::Zlib => Zlib::read_block_info(reader, orig_file_size),
            #[cfg(feature = "lzvn")]
            Kind::Lzvn => Lzvn::read_block_info(reader, orig_file_size),
            #[cfg(feature = "lzfse")]
            Kind::Lzfse => Lzfse::read_block_info(reader, orig_file_size),
            #[allow(unreachable_patterns)]
            _ => panic!("Unsupported compression kind {self}"),
        }
    }

    pub fn finish<W: io::Write + io::Seek>(self, writer: W, block_sizes: &[u32]) -> io::Result<()> {
        match self {
            #[cfg(feature = "zlib")]
            Kind::Zlib => Zlib::finish(writer, block_sizes),
            #[cfg(feature = "lzvn")]
            Kind::Lzvn => Lzvn::finish(writer, block_sizes),
            #[cfg(feature = "lzfse")]
            Kind::Lzfse => Lzfse::finish(writer, block_sizes),
            #[allow(unreachable_patterns)]
            _ => panic!("Unsupported compression kind {self}"),
        }
    }
}

impl Default for Kind {
    #[inline]
    fn default() -> Self {
        if Self::Lzfse.supported() {
            Self::Lzfse
        } else if Self::Zlib.supported() {
            Self::Zlib
        } else {
            Self::Lzvn
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAINTEXT: &[u8] = include_bytes!("mod.rs");

    pub(super) fn compressor_round_trip<C: CompressorImpl>(c: &mut C) {
        let mut buf = vec![0u8; PLAINTEXT.len() * 2];
        let len = c.compress(&mut buf, PLAINTEXT, 6).unwrap();
        assert!(len > 0);
        assert!(len < buf.len());
        let ciphertext = &buf[..len];
        let mut buf = vec![0u8; PLAINTEXT.len() + 1];
        let len = c.decompress(&mut buf, ciphertext).unwrap();
        assert_eq!(&buf[..len], PLAINTEXT);
    }
}
