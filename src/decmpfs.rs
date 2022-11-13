use crate::compressor;
use std::ffi::CStr;
use std::{fmt, io, mem};

pub const MAX_XATTR_SIZE: usize = 3802;
pub const MAX_XATTR_DATA_SIZE: usize = MAX_XATTR_SIZE - DiskHeader::SIZE;
pub const MAGIC: [u8; 4] = *b"fpmc";

pub const ZLIB_BLOCK_TABLE_START: u64 = 0x104;

pub const XATTR_NAME: &CStr = crate::cstr!("com.apple.decmpfs");

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Storage {
    Xattr,
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

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CompressionType(u32);

impl fmt::Display for CompressionType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.compression_storage() {
            Some((compressor, storage)) => {
                write!(f, "{} in {}", compressor, storage)
            }
            None => write!(f, "unknown compression type: {}", self.0),
        }
    }
}

impl CompressionType {
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

    pub const fn from_raw_type(n: u32) -> Self {
        Self(n)
    }

    pub const fn raw_type(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Copy, Clone)]
pub struct DiskHeader {
    pub compression_type: u32,
    pub uncompressed_size: u64,
}

impl DiskHeader {
    pub const SIZE: usize = 16;

    pub fn write_into<W: io::Write>(&self, mut w: W) -> io::Result<()> {
        w.write_all(&MAGIC)?;
        w.write_all(&self.compression_type.to_le_bytes())?;
        w.write_all(&self.uncompressed_size.to_le_bytes())?;
        Ok(())
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

#[derive(Debug, Copy, Clone)]
pub struct ZlibBlockInfo {
    pub offset: u32,
    pub compressed_size: u32,
}

impl ZlibBlockInfo {
    pub const SIZE: u64 = mem::size_of::<u32>() as u64 * 2;

    pub fn write_into<W: io::Write>(self, mut w: W) -> io::Result<()> {
        w.write_all(&u32::to_le_bytes(self.offset))?;
        w.write_all(&u32::to_le_bytes(self.compressed_size))?;
        Ok(())
    }
}
