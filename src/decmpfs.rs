use crate::compressor;
use std::ffi::CStr;
use std::{io, mem};

pub const MAX_XATTR_SIZE: usize = 3802;
pub const MAGIC: u32 = u32::from_be_bytes(*b"cmpf");

pub const ZLIB_BLOCK_TABLE_START: u64 = 0x104;

pub const XATTR_NAME: &CStr = crate::cstr!("com.apple.decmpfs");

#[derive(Copy, Clone)]
pub enum Storage {
    Xattr,
    ResourceFork,
}

#[derive(Copy, Clone)]
pub struct CompressionType {
    pub compression: compressor::Kind,
    pub storage: Storage,
}

impl CompressionType {
    pub fn raw_type(self) -> u32 {
        match self {
            CompressionType {
                compression: compressor::Kind::Zlib,
                storage: Storage::Xattr,
            } => 3,
            CompressionType {
                compression: compressor::Kind::Zlib,
                storage: Storage::ResourceFork,
            } => 4,

            CompressionType {
                compression: compressor::Kind::Lzvn,
                storage: Storage::Xattr,
            } => 7,
            CompressionType {
                compression: compressor::Kind::Lzvn,
                storage: Storage::ResourceFork,
            } => 8,

            CompressionType {
                compression: compressor::Kind::Lzfse,
                storage: Storage::Xattr,
            } => 11,
            CompressionType {
                compression: compressor::Kind::Lzfse,
                storage: Storage::ResourceFork,
            } => 12,
        }
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
        w.write_all(&MAGIC.to_le_bytes())?;
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
