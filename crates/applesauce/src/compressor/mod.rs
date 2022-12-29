#[cfg(feature = "lzfse")]
use crate::compressor::lzfse::Lzfse;
#[cfg(feature = "lzvn")]
use crate::compressor::lzvn::Lzvn;
#[cfg(feature = "zlib")]
use crate::compressor::zlib::Zlib;
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
    fn blocks_start(block_count: u64) -> u64;

    fn compress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize>;
    fn decompress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize>;

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
        self.kind().blocks_start(block_count)
    }

    pub fn compress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize> {
        match self.0 {
            #[cfg(feature = "zlib")]
            Data::Zlib(ref mut i) => i.compress(dst, src),
            #[cfg(feature = "lzfse")]
            Data::Lzfse(ref mut i) => i.compress(dst, src),
            #[cfg(feature = "lzvn")]
            Data::Lzvn(ref mut i) => i.compress(dst, src),
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
pub enum Kind {
    Zlib,
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
    pub const fn supported(self) -> bool {
        // Clippy falsely sees these arms as identical
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
    pub fn blocks_start(self, block_count: u64) -> u64 {
        match self {
            #[cfg(feature = "zlib")]
            Kind::Zlib => Zlib::blocks_start(block_count),
            #[cfg(feature = "lzvn")]
            Kind::Lzvn => Lzvn::blocks_start(block_count),
            #[cfg(feature = "lzfse")]
            Kind::Lzfse => Lzfse::blocks_start(block_count),
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAINTEXT: &[u8] = include_bytes!("mod.rs");

    pub(super) fn compressor_round_trip<C: CompressorImpl>(c: &mut C) {
        let mut buf = vec![0u8; PLAINTEXT.len() * 2];
        let len = c.compress(&mut buf, PLAINTEXT).unwrap();
        assert!(len > 0);
        assert!(len < buf.len());
        let ciphertext = &buf[..len];
        let mut buf = vec![0u8; PLAINTEXT.len() + 1];
        let len = c.decompress(&mut buf, ciphertext).unwrap();
        assert_eq!(&buf[..len], PLAINTEXT);
    }
}
