use std::fs::File;
use std::io;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn chrome_tracing_file(path: Option<&Path>) -> Option<impl io::Write> {
    let path = path?;

    let file = match File::create(path) {
        Ok(file) => file,
        Err(e) => {
            // Tracing isn't set up yet, log the old-fashioned way
            eprintln!("Unable to open chrome layer: {e}");
            return None;
        }
    };

    Some(BufWriter::new(file))
}

#[tokio::main]
async fn main() {
    let mut _chrome_guard = None;
    let chrome_file = chrome_tracing_file(Some(Path::new("/tmp/trace.json")));
    let chrome_layer: Option<_> = chrome_file.map(|f| {
        let (layer, guard) = ChromeLayerBuilder::new()
            .writer(f)
            .include_args(true)
            .build();
        _chrome_guard = Some(guard);
        layer
    });

    tracing_subscriber::registry()
        .with(chrome_layer)
        // .with(fmt::layer().with_span_events(FmtSpan::FULL))
        .init();

    let start = Instant::now();
    let path = PathBuf::from("/tmp/dir/zeros");
    let metadata = tokio::fs::metadata(&path).await.unwrap();
    applesauce::block_stream::compress_file(path, metadata)
        .await
        .unwrap();
    dbg!(start.elapsed());
}
