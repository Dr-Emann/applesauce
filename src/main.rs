use applesauce::compressor::Compressor;
use std::path::Path;

fn main() {
    let path = Path::new("/tmp/file");

    let mut compressor = applesauce::FileCompressor::new(Compressor::lzfse());
    match compressor.compress_path(path) {
        Ok(()) => {}
        Err(e) => eprintln!("Error compressing {}: {}", path.display(), e),
    }
}
