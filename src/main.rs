use squashed_apple::compressor::Compressor;
use std::path::Path;

fn main() {
    let path = Path::new("/tmp/file");
    let metadata = path.metadata().unwrap();

    let mut compressor = Compressor::lzfse();

    match squashed_apple::compress(path, &metadata, &mut compressor) {
        Ok(()) => {}
        Err(e) => eprintln!("Error compressing {}: {}", path.display(), e),
    }
}
