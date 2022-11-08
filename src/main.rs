use crate::cli_progress::ProgressBars;
use applesauce::Compressor;
use cfg_if::cfg_if;
use clap::Parser;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::fmt::time;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use walkdir::WalkDir;

mod cli_progress;

#[derive(Debug, clap::Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Compress files
    Compress(Compress),
}

#[derive(Debug, clap::Args)]
struct Compress {
    /// Paths to recursively compress
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// The type of compression to use
    #[arg(short, long, value_enum, default_value_t = Compression::default())]
    compression: Compression,
}

#[derive(Debug, Copy, Clone, clap::ValueEnum)]
enum Compression {
    #[cfg(feature = "lzfse")]
    Lzfse,
    #[cfg(feature = "zlib")]
    Zlib,
    #[cfg(feature = "lzvn")]
    Lzvn,
}

impl Compression {
    fn compressor(self) -> Compressor {
        match self {
            #[cfg(feature = "zlib")]
            Compression::Zlib => Compressor::zlib(),
            #[cfg(feature = "lzfse")]
            Compression::Lzfse => Compressor::lzfse(),
            #[cfg(feature = "lzvn")]
            Compression::Lzvn => Compressor::lzvn(),
        }
    }
}

impl Default for Compression {
    fn default() -> Self {
        cfg_if! {
            if #[cfg(feature = "lzfse")] {
                Self::Lzfse
            } else if #[cfg(feature = "zlib")] {
                Self::Zlib
            } else if #[cfg(feature = "lzvn")] {
                Self::Lzvn
            } else {
                compile_error!("At least one compression type must be configured")
            }
        }
    }
}

fn main() {
    let cli = Cli::parse();

    let (chrome_layer, _guard) = ChromeLayerBuilder::new()
        .file("/tmp/trace.json")
        .include_args(true)
        .build();
    let fmt_layer = tracing_subscriber::fmt::layer().with_timer(time::uptime());
    tracing_subscriber::registry()
        .with(chrome_layer)
        .with(fmt_layer)
        .init();

    match cli.command {
        Commands::Compress(Compress { paths, compression }) => {
            let progress_bars = ProgressBars::new();
            let mut compressor = applesauce::FileCompressor::new(compression.compressor());
            paths
                .iter()
                .flat_map(|root| WalkDir::new(&root).into_iter().map(move |x| (x, root)))
                .for_each(|(entry, root)| {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(e) => {
                            tracing::error!("{e}");
                            return;
                        }
                    };

                    if !entry.file_type().is_file() {
                        return;
                    }

                    let full_path = root.join(entry.path());
                    let truncated_path = truncate_path(&full_path, progress_bars.prefix_len());
                    let pb = progress_bars.add(truncated_path.display().to_string());

                    compressor.compress_path(full_path, pb);
                });
            drop(compressor);
            progress_bars.finish();
            tracing::info!("Finished compressing");
        }
    }
}

pub fn truncate_path(path: &Path, width: usize) -> PathBuf {
    let mut segments: Vec<_> = path.components().collect();
    let mut total_len = path.as_os_str().len();

    if total_len <= width || segments.len() <= 1 {
        return path.to_owned();
    }

    let mut first = true;
    while total_len > width && segments.len() > 1 {
        // Bias toward the beginning for even counts
        let mid = (segments.len() - 1) / 2;
        let segment = segments[mid];
        if matches!(segment, Component::RootDir | Component::Prefix(_)) {
            break;
        }

        total_len -= segment.as_os_str().len();

        if first {
            // First time, we're just replacing the segment with an ellipsis
            // like `aa/bb/cc/dd` -> `aa/…/cc/dd`, so we remove the
            // segment, and add an ellipsis char
            total_len += 1;
            first = false;
        } else {
            // Other times, we're removing the segment, and a slash
            // `aa/…/cc/dd` -> `aa/…/dd`
            total_len -= 1;
        }
        segments.remove(mid);
    }
    segments.insert(segments.len() / 2, Component::Normal(OsStr::new("…")));
    let mut path = PathBuf::with_capacity(total_len);
    for segment in segments {
        path.push(segment);
    }

    path
}

#[test]
fn minimal_truncate() {
    let orig_path = Path::new("abcd");
    // Trying to truncate smaller than a single segment does nothing
    assert_eq!(truncate_path(orig_path, 1), PathBuf::from("abcd"));

    let orig_path = Path::new("1234/5678");
    // Trying to truncate removes the first element
    assert_eq!(truncate_path(orig_path, 1), PathBuf::from("…/5678"));
    let orig_path = Path::new("/1234/5678");
    // Never truncate the leading /
    assert_eq!(truncate_path(orig_path, 1), PathBuf::from("/…/5678"));

    let orig_path = Path::new("/1234/5678");
    assert_eq!(truncate_path(orig_path, 1), PathBuf::from("/…/5678"));

    let orig_path = Path::new("/1234/5678/90123/4567");
    assert_eq!(truncate_path(orig_path, 1), PathBuf::from("/…/4567"));
}

#[test]
fn no_truncation() {
    let orig_path = Path::new("abcd");
    assert_eq!(truncate_path(orig_path, 4), PathBuf::from(orig_path));

    let orig_path = Path::new("a/b/c/d");
    assert_eq!(truncate_path(orig_path, 7), PathBuf::from(orig_path));
    let orig_path = Path::new("/a/b/c/d");
    assert_eq!(truncate_path(orig_path, 8), PathBuf::from(orig_path));
}

#[test]
fn truncate_single_segment() {
    let orig_path = Path::new("a/bbbbbbbbbb/c");
    assert_eq!(truncate_path(orig_path, 5), PathBuf::from("a/…/c"));
}

#[test]
fn command_check() {
    use clap::CommandFactory;
    Cli::command().debug_assert()
}
