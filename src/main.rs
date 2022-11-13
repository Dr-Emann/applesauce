use crate::cli_progress::{ProgressBarWriter, ProgressBars};
use applesauce::{info, Compressor};
use cfg_if::cfg_if;
use clap::Parser;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufWriter, LineWriter};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use tracing::metadata::LevelFilter;
use tracing::Level;
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::fmt::time;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;
use walkdir::WalkDir;

mod cli_progress;

#[derive(Debug, clap::Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output chrome tracing format to a file
    ///
    /// The passed file can be passed to chrome at chrome://tracing
    #[arg(long, global(true))]
    chrome_tracing: Option<PathBuf>,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Compress files
    Compress(Compress),
    /// Get info about compression for file(s)
    Info(Info),
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

#[derive(Debug, clap::Args)]
struct Info {
    /// Paths to inspect
    ///
    /// Info will be reported for each path
    #[arg(required = true)]
    paths: Vec<PathBuf>,
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

    let _chrome_guard;
    let chrome_layer = 'layer: {
        match cli.chrome_tracing {
            Some(path) => {
                let file = match File::create(path) {
                    Ok(file) => file,
                    Err(e) => {
                        tracing::error!("Unable to open chrome layer: {e}");
                        break 'layer None;
                    }
                };
                let writer = {
                    cfg_if! {
                        if #[cfg(feature = "zlib")] {
                            flate2::write::GzEncoder::new(file, flate2::Compression::default())
                        } else {
                            file
                        }
                    }
                };
                let (layer, guard) = ChromeLayerBuilder::new()
                    .writer(BufWriter::new(writer))
                    .include_args(true)
                    .build();
                _chrome_guard = guard;
                Some(layer)
            }
            None => None,
        }
    };

    let progress_bars = ProgressBars::new();
    let fmt_writer = Mutex::new(LineWriter::new(ProgressBarWriter::new(
        progress_bars.multi_progress().clone(),
        std::io::stderr(),
    )));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_timer(time::uptime())
        .with_writer(fmt_writer)
        .with_filter(LevelFilter::from_level(Level::INFO));
    tracing_subscriber::registry()
        .with(chrome_layer)
        .with(fmt_layer)
        .init();

    match cli.command {
        Commands::Compress(Compress { paths, compression }) => {
            let mut compressor = applesauce::FileCompressor::new(compression.compressor());
            paths
                .iter()
                .flat_map(|root| WalkDir::new(root).into_iter())
                .for_each(|entry| {
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

                    let truncated_path = truncate_path(entry.path(), progress_bars.prefix_len());
                    let pb = progress_bars.add(truncated_path.display().to_string());

                    compressor.compress_path(entry.path().to_owned(), pb);
                });
            drop(compressor);
            progress_bars.finish();
            tracing::info!("Finished compressing");
        }
        Commands::Info(info) => {
            for path in info.paths {
                let info = info::get(&path);
                let info = match info {
                    Ok(info) => info,
                    Err(e) => {
                        tracing::error!(
                            "error reading compression info for {}: {}",
                            path.display(),
                            e,
                        );
                        continue;
                    }
                };
                if info.is_compressed {
                    println!("{} is compressed", path.display());
                } else {
                    println!("{} is not compressed", path.display());
                }

                match &info.decmpfs_info {
                    Some(Ok(decmpfs_info)) => {
                        println!("Compression type: {}", decmpfs_info.compression_type);
                        println!(
                            "Uncompressed size in decmpfs xattr: {}",
                            decmpfs_info.orig_file_size
                        );
                    }
                    Some(Err(decmpfs_err)) => {
                        if info.is_compressed {
                            tracing::error!(
                                "compressed file has issue with decompfs xattr: {}",
                                decmpfs_err
                            );
                        }
                    }
                    None => {
                        if info.is_compressed {
                            tracing::error!("compressed file has no decmpfs xattr");
                        }
                    }
                }
                println!("Uncompressed size: {}", info.stat_size);
                if info.is_compressed {
                    println!("Compressed size: {}", info.on_disk_size);
                    println!(
                        "Compression savings: {:0.2}%",
                        (1.0 - info.compressed_fraction()) * 100.0
                    )
                }
                println!("Number of extended attributes: {}", info.xattr_count);
                println!(
                    "Size of extended attributes: {} bytes",
                    info.total_xattr_size
                );
            }
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
