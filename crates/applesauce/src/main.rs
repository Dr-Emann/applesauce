use std::path::PathBuf;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let start = Instant::now();
    let path = PathBuf::from("/tmp/dir/zeros");
    let metadata = tokio::fs::metadata(&path).await.unwrap();
    applesauce::block_stream::compress_file(path, metadata)
        .await
        .unwrap();
    dbg!(start.elapsed());
}
