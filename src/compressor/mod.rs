use crate::compressor::lzfse::Lzfse;
use crate::compressor::lzvn::Lzvn;
use crate::compressor::zlib::Zlib;
use std::io;

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
    fn blocks_start(&self, block_count: u64) -> u64;

    fn compress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize>;
    fn decompress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize>;

    fn finish<W: io::Write + io::Seek>(&mut self, writer: W, block_sizes: &[u32])
        -> io::Result<()>;
}

pub struct Compressor(Data);

impl Compressor {
    #[cfg(feature = "zlib")]
    pub fn zlib() -> Self {
        Self(Data::Zlib(Zlib))
    }

    #[cfg(feature = "lzfse")]
    pub fn lzfse() -> Self {
        Self(Data::Lzfse(Lzfse::new()))
    }

    #[cfg(feature = "lzvn")]
    pub fn lzvn() -> Self {
        Self(Data::Lzvn(Lzvn::new()))
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

impl Data {
    pub fn kind(&self) -> Kind {
        match self {
            #[cfg(feature = "zlib")]
            Data::Zlib(_) => Kind::Zlib,
            #[cfg(feature = "lzfse")]
            Data::Lzfse(_) => Kind::Lzfse,
            #[cfg(feature = "lzvn")]
            Data::Lzvn(_) => Kind::Lzvn,
        }
    }
}

impl CompressorImpl for Compressor {
    fn blocks_start(&self, block_count: u64) -> u64 {
        match &self.0 {
            #[cfg(feature = "zlib")]
            Data::Zlib(i) => i.blocks_start(block_count),
            #[cfg(feature = "lzfse")]
            Data::Lzfse(i) => i.blocks_start(block_count),
            #[cfg(feature = "lzvn")]
            Data::Lzvn(i) => i.blocks_start(block_count),
        }
    }

    fn compress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize> {
        match &mut self.0 {
            #[cfg(feature = "zlib")]
            Data::Zlib(i) => i.compress(dst, src),
            #[cfg(feature = "lzfse")]
            Data::Lzfse(i) => i.compress(dst, src),
            #[cfg(feature = "lzvn")]
            Data::Lzvn(i) => i.compress(dst, src),
        }
    }

    fn decompress(&mut self, dst: &mut [u8], src: &[u8]) -> io::Result<usize> {
        match &mut self.0 {
            #[cfg(feature = "zlib")]
            Data::Zlib(i) => i.decompress(dst, src),
            #[cfg(feature = "lzfse")]
            Data::Lzfse(i) => i.decompress(dst, src),
            #[cfg(feature = "lzvn")]
            Data::Lzvn(i) => i.decompress(dst, src),
        }
    }

    fn finish<W: io::Write + io::Seek>(
        &mut self,
        writer: W,
        block_sizes: &[u32],
    ) -> io::Result<()> {
        match &mut self.0 {
            #[cfg(feature = "zlib")]
            Data::Zlib(i) => i.finish(writer, block_sizes),
            #[cfg(feature = "lzfse")]
            Data::Lzfse(i) => i.finish(writer, block_sizes),
            #[cfg(feature = "lzvn")]
            Data::Lzvn(i) => i.finish(writer, block_sizes),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Kind {
    Zlib,
    Lzvn,
    Lzfse,
}

impl Kind {
    #[must_use]
    pub fn supported(self) -> bool {
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
