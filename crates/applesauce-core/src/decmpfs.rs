//! Helpers for working with decmpfs xattrs

use crate::compressor;
use std::ffi::CStr;
use std::io::Write;
use std::{fmt, io};

/// The length of the decmpfs xattr header
pub const HEADER_LEN: usize = 16;
/// The maximum size of a decmpfs xattr
pub const MAX_XATTR_SIZE: usize = 3802;
/// The maximum size of the data in a decmpfs xattr (following the header)
pub const MAX_XATTR_DATA_SIZE: usize = MAX_XATTR_SIZE - HEADER_LEN;
/// The magic bytes that identify a decmpfs xattr
pub const MAGIC: [u8; 4] = *b"fpmc";

pub const ZLIB_BLOCK_TABLE_START: u64 = 0x104;

/// The name of the decmpfs xattr
pub const XATTR_NAME: &CStr = {
    let bytes: &'static [u8] = b"com.apple.decmpfs\0";
    // SAFETY: bytes are static, and null terminated, without internal nulls
    unsafe { CStr::from_bytes_with_nul_unchecked(bytes) }
};

/// The location of the compressed data
///
/// Compressed data can be stored either in the decmpfs xattr or in the resource fork.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Storage {
    /// The decmpfs header is followed by a single compressed block
    Xattr,
    /// The compressed data is stored separately, in the resource fork
    ResourceFork,
}

impl fmt::Display for Storage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            Storage::Xattr => "decmpfs xattr",
            Storage::ResourceFork => "resource fork",
        };
        f.write_str(s)
    }
}

/// A combination of the compressor kind, and where the compressed data is stored
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct CompressionType(u32);

impl fmt::Display for CompressionType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.compression_storage() {
            Some((compressor, storage)) => {
                write!(f, "{compressor} in {storage}")
            }
            None => write!(f, "unknown compression type: {}", self.0),
        }
    }
}

impl CompressionType {
    #[must_use]
    #[inline]
    pub const fn new(compressor: compressor::Kind, storage: Storage) -> Self {
        let val = match (compressor, storage) {
            (compressor::Kind::Zlib, Storage::Xattr) => 3,
            (compressor::Kind::Zlib, Storage::ResourceFork) => 4,
            (compressor::Kind::Lzvn, Storage::Xattr) => 7,
            (compressor::Kind::Lzvn, Storage::ResourceFork) => 8,
            (compressor::Kind::Lzfse, Storage::Xattr) => 11,
            (compressor::Kind::Lzfse, Storage::ResourceFork) => 12,
        };
        Self(val)
    }

    #[must_use]
    #[inline]
    pub const fn compression_storage(self) -> Option<(compressor::Kind, Storage)> {
        match self.0 {
            3 => Some((compressor::Kind::Zlib, Storage::Xattr)),
            4 => Some((compressor::Kind::Zlib, Storage::ResourceFork)),
            7 => Some((compressor::Kind::Lzvn, Storage::Xattr)),
            8 => Some((compressor::Kind::Lzvn, Storage::ResourceFork)),
            11 => Some((compressor::Kind::Lzfse, Storage::Xattr)),
            12 => Some((compressor::Kind::Lzfse, Storage::ResourceFork)),
            _ => None,
        }
    }

    #[must_use]
    #[inline]
    pub const fn from_raw_type(n: u32) -> Self {
        Self(n)
    }

    #[must_use]
    #[inline]
    pub const fn raw_type(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DecodeError {
    TooSmall,
    BadMagic,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match *self {
            DecodeError::TooSmall => "decmpfs xattr too small to hold compression header",
            DecodeError::BadMagic => "decmpfs xattr magic field has incorrect value",
        };
        f.write_str(s)
    }
}

impl std::error::Error for DecodeError {}

impl From<DecodeError> for io::Error {
    fn from(err: DecodeError) -> Self {
        match err {
            DecodeError::TooSmall => io::Error::new(io::ErrorKind::UnexpectedEof, err),
            DecodeError::BadMagic => io::Error::new(io::ErrorKind::InvalidData, err),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Value<'a> {
    pub compression_type: CompressionType,
    pub uncompressed_size: u64,
    pub extra_data: &'a [u8],
}

#[allow(clippy::len_without_is_empty)]
impl<'a> Value<'a> {
    pub fn from_data(data: &'a [u8]) -> Result<Self, DecodeError> {
        if data.len() < HEADER_LEN {
            return Err(DecodeError::TooSmall);
        }
        let (header, extra_data) = data.split_at(HEADER_LEN);
        let magic = &header[0..4];
        let compression_type = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let uncompressed_size = u64::from_le_bytes(header[8..16].try_into().unwrap());
        if magic != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        let compression_type = CompressionType::from_raw_type(compression_type);

        Ok(Self {
            compression_type,
            uncompressed_size,
            extra_data,
        })
    }

    pub fn write_to<W: Write>(self, mut writer: W) -> io::Result<()> {
        writer.write_all(&self.header_bytes())?;
        writer.write_all(self.extra_data)?;

        Ok(())
    }

    fn header_bytes(self) -> [u8; HEADER_LEN] {
        let mut result = [0; HEADER_LEN];

        let mut writer = &mut result[..];
        writer.write_all(&MAGIC).unwrap();
        writer
            .write_all(&self.compression_type.0.to_le_bytes())
            .unwrap();
        writer
            .write_all(&self.uncompressed_size.to_le_bytes())
            .unwrap();
        assert!(writer.is_empty());

        result
    }

    pub fn len(self) -> usize {
        HEADER_LEN + self.extra_data.len()
    }
}

#[rustfmt::skip]
pub const ZLIB_TRAILER: [u8; 50] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // magic 1 + 2
    0x00, 0x1C, 0x00, 0x32,
    // spacer1
    0x00, 0x00,
    // compression_magic
    b'c', b'm', b'p', b'f',
    // magic3
    0x00, 0x00, 0x00, 0x0A,
    // magic4
    0x00, 0x01, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00,
    // spacer2
    0x00, 0x00, 0x00, 0x00,
];

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BlockInfo {
    pub offset: u32,
    pub compressed_size: u32,
}

impl BlockInfo {
    pub const SIZE: usize = 8;

    #[inline]
    pub fn from_bytes(data: [u8; Self::SIZE]) -> Self {
        let offset = u32::from_le_bytes(data[..4].try_into().unwrap());
        let compressed_size = u32::from_le_bytes(data[4..].try_into().unwrap());
        Self {
            offset,
            compressed_size,
        }
    }

    #[inline]
    pub fn to_bytes(self) -> [u8; Self::SIZE] {
        let mut result = [0; Self::SIZE];

        let Self {
            offset,
            compressed_size,
        } = self;

        let mut writer = &mut result[..];
        writer.write_all(&offset.to_le_bytes()).unwrap();
        writer.write_all(&compressed_size.to_le_bytes()).unwrap();
        assert!(writer.is_empty());

        result
    }
}
