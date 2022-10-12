use std::path::Path;

fn main() {
    let path = Path::new("/tmp/file");
    let metadata = path.metadata().unwrap();

    match squashed_apple::compress(path, &metadata) {
        Ok(()) => {}
        Err(e) => eprintln!("Error compressing {}: {}", path.display(), e),
    }
}
