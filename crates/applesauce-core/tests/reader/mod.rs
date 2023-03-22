use applesauce_core::compressor::Kind;
use applesauce_core::decmpfs;
use applesauce_core::reader::Reader;
use std::io::Cursor;

fn never_called_open() -> Cursor<Vec<u8>> {
    panic!("Should not be called");
}

#[test]
fn empty() {
    let reader_err = Reader::new(&[], never_called_open).unwrap_err();
    assert_eq!(reader_err.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn invalid_magic() {
    let reader_err = Reader::new(&[0; decmpfs::HEADER_LEN], never_called_open).unwrap_err();
    assert_eq!(reader_err.kind(), std::io::ErrorKind::InvalidData);
}

fn round_trip(kind: Kind, uncompressed_data: &[u8]) {
    let mut compressor = kind.compressor().unwrap();

    let mut resource_fork = Vec::new();
    let mut writer = applesauce_core::writer::Writer::new(kind, uncompressed_data.len() as u64, {
        let rfork_ref = &mut resource_fork;
        move || Cursor::new(rfork_ref)
    })
    .unwrap();

    let mut compressed_block = vec![0; applesauce_core::BLOCK_SIZE * 2];
    for block in uncompressed_data.chunks(applesauce_core::BLOCK_SIZE) {
        let len = compressor
            .compress(&mut compressed_block, block, 5)
            .unwrap();
        writer.add_block(&compressed_block[..len]).unwrap();
    }

    let mut decmpfs_data = Vec::new();
    writer.finish_decmpfs_data(&mut decmpfs_data).unwrap();

    let mut reader = Reader::new(&decmpfs_data, || Cursor::new(&resource_fork)).unwrap();

    assert_eq!(reader.compression_kind(), kind);
    assert_eq!(
        reader.remaining_blocks(),
        applesauce_core::num_blocks(uncompressed_data.len() as u64) as usize
    );

    let mut clear_data = Vec::new();
    // Need an extra byte, because lzfse/lzvn needs at least one extra byte to differentiate between
    // finishing on the last byte and running out of space
    let mut clear_buf = vec![0; applesauce_core::BLOCK_SIZE + 1];

    loop {
        compressed_block.clear();
        if !reader.read_block_into(&mut compressed_block).unwrap() {
            break;
        }
        let len = compressor
            .decompress(&mut clear_buf, &compressed_block)
            .unwrap();
        let expected_len = if reader.remaining_blocks() != 0
            || uncompressed_data.len() % applesauce_core::BLOCK_SIZE == 0
        {
            applesauce_core::BLOCK_SIZE
        } else {
            uncompressed_data.len() % applesauce_core::BLOCK_SIZE
        };
        assert_eq!(len, expected_len);
        clear_data.extend_from_slice(&clear_buf[..len]);
    }
    assert_eq!(clear_data, uncompressed_data);
}

macro_rules! round_trip_tests {
    ($($name:ident),* $(,)?) => {
        $(
            mod $name {
                use super::round_trip;
                use applesauce_core::compressor::Compressor;

                #[test]
                fn round_trip_empty() {
                    round_trip(Compressor::$name().kind(), &[]);
                }

                #[test]
                fn round_trip_small() {
                    round_trip(Compressor::$name().kind(), &[1]);
                    round_trip(Compressor::$name().kind(), &[1, 2, 3, 4]);
                }

                #[test]
                fn round_trip_large_compresable() {
                    round_trip(Compressor::$name().kind(), &[1; 1024 * 1024]);
                    round_trip(Compressor::$name().kind(), &[1; 1024 * 1024 - 1]);
                }

                #[test]
                fn round_trip_large_rand() {
                    use rand::RngCore;

                    let mut data = vec![0; 1024 * 1024];
                    rand::thread_rng().fill_bytes(&mut data);

                    round_trip(Compressor::$name().kind(), &data);
                }
            }
        )*
    };
}

#[cfg(feature = "lzfse")]
round_trip_tests!(lzfse);

#[cfg(feature = "lzvn")]
round_trip_tests!(lzvn);

#[cfg(feature = "zlib")]
round_trip_tests!(zlib);
