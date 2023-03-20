use applesauce_core::compressor::Kind;
use applesauce_core::writer::Writer;
use applesauce_core::{decmpfs, BLOCK_SIZE};
use std::io::Cursor;

fn never_called_open() -> Cursor<Vec<u8>> {
    panic!("Should not be called");
}

#[test]
fn empty() {
    let writer = Writer::new(Kind::default(), 0, never_called_open).unwrap();

    let mut decmpfs_data = Vec::new();
    writer.finish_decmpfs_data(&mut decmpfs_data).unwrap();

    let value = decmpfs::Value::from_data(&decmpfs_data).unwrap();
    let (kind, storage) = value.compression_type.compression_storage().unwrap();
    assert_eq!(kind, Kind::default());
    assert_eq!(storage, decmpfs::Storage::Xattr);
    assert!(value.extra_data.is_empty());
}

#[test]
fn small_block_store_inplace() {
    // Uses a single block to store this much data
    let uncompressed_size = 10;
    let compressed_block = vec![1, 2, 3];
    let mut writer = Writer::new(Kind::default(), uncompressed_size, never_called_open).unwrap();
    writer.add_block(compressed_block.clone()).unwrap();

    let mut decmpfs_data = Vec::new();
    writer.finish_decmpfs_data(&mut decmpfs_data).unwrap();

    let value = decmpfs::Value::from_data(&decmpfs_data).unwrap();
    let (kind, storage) = value.compression_type.compression_storage().unwrap();
    assert_eq!(kind, Kind::default());
    assert_eq!(storage, decmpfs::Storage::Xattr);
    assert_eq!(value.extra_data, compressed_block);
}

#[test]
fn large_single_block() {
    // Treat a single block which can't fit in the decmpfs xattr when compressed the same way we treat
    // multiple blocks

    let uncompressed_size = BLOCK_SIZE as u64;
    let compressed_block = vec![0x1A; decmpfs::MAX_XATTR_DATA_SIZE + 1];
    let mut resource_fork = Vec::new();

    let mut writer = {
        let rfork_ref = &mut resource_fork;
        Writer::new(Kind::default(), uncompressed_size, move || {
            Cursor::new(rfork_ref)
        })
        .unwrap()
    };

    writer.add_block(compressed_block.clone()).unwrap();

    let mut decmpfs_data = Vec::new();
    writer.finish_decmpfs_data(&mut decmpfs_data).unwrap();

    let value = decmpfs::Value::from_data(&decmpfs_data).unwrap();
    let (kind, storage) = value.compression_type.compression_storage().unwrap();
    assert_eq!(kind, Kind::default());
    assert_eq!(storage, decmpfs::Storage::ResourceFork);
    assert!(value.extra_data.is_empty());

    let block_infos = kind
        .read_block_info(Cursor::new(&resource_fork), uncompressed_size)
        .unwrap();
    assert_eq!(
        block_infos,
        [decmpfs::BlockInfo {
            offset: kind.header_size(1) as u32,
            compressed_size: compressed_block.len() as u32,
        }]
    );
    assert_eq!(
        &resource_fork[kind.header_size(1) as usize..][..compressed_block.len()],
        compressed_block
    );
}

#[test]
fn multiple_small_blocks() {
    let uncompressed_size = 2 * BLOCK_SIZE as u64;
    let compressed_block = vec![0x1A; 10];
    let mut resource_fork = Vec::new();

    let mut writer = {
        let rfork_ref = &mut resource_fork;
        Writer::new(Kind::default(), uncompressed_size, move || {
            Cursor::new(rfork_ref)
        })
        .unwrap()
    };

    writer.add_block(compressed_block.clone()).unwrap();
    writer.add_block(compressed_block.clone()).unwrap();

    let mut decmpfs_data = Vec::new();
    writer.finish_decmpfs_data(&mut decmpfs_data).unwrap();

    let value = decmpfs::Value::from_data(&decmpfs_data).unwrap();
    let (kind, storage) = value.compression_type.compression_storage().unwrap();
    assert_eq!(kind, Kind::default());
    assert_eq!(storage, decmpfs::Storage::ResourceFork);
    assert!(value.extra_data.is_empty());

    let block_infos = kind
        .read_block_info(Cursor::new(&resource_fork), uncompressed_size)
        .unwrap();
    let header_size = kind.header_size(2) as u32;
    assert_eq!(
        block_infos,
        [
            decmpfs::BlockInfo {
                offset: header_size,
                compressed_size: compressed_block.len() as u32,
            },
            decmpfs::BlockInfo {
                offset: (header_size as usize + compressed_block.len()) as u32,
                compressed_size: compressed_block.len() as u32,
            },
        ]
    );
    let resource_fork_data = &resource_fork[header_size as usize..];
    let resource_fork_data = &resource_fork_data[..compressed_block.len() * 2];
    assert_eq!(
        &resource_fork_data[..compressed_block.len()],
        compressed_block
    );
    assert_eq!(
        &resource_fork_data[compressed_block.len()..],
        compressed_block
    );
}
