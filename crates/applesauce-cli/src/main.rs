use crate::progress::{ProgressBarWriter, ProgressBars, Verbosity};
use applesauce::compressor::Kind;
use applesauce::{compressor, info, Stats};
use cfg_if::cfg_if;
use clap::Parser;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufWriter, LineWriter};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::{fmt, io};
use tracing::metadata::LevelFilter;
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::fmt::time;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

mod progress;

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

    #[arg(short, long, global(true), action = clap::ArgAction::Count)]
    verbose: u8,

    #[arg(short, long, global(true), action = clap::ArgAction::Count, conflicts_with = "verbose")]
    quiet: u8,
}

impl Cli {
    fn verbosity(&self) -> Verbosity {
        let verbosity = self.verbose as i8 - self.quiet as i8;
        match verbosity {
            ..=-1 => Verbosity::Quiet,
            0 => Verbosity::Normal,
            1.. => Verbosity::Verbose,
        }
    }
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Compress files
    Compress(Compress),

    /// Decompress files
    Decompress(Decompress),

    /// Get info about compression for file(s)
    Info(Info),
}

#[derive(Debug, clap::Args)]
struct Decompress {
    /// Paths to recursively decompress
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// Decompress manually, rather than allowing the OS to do decompression
    ///
    /// This may be useful to do decompression from an older OS version which can't
    /// natively read the compressed file
    #[arg(long)]
    manual: bool,

    /// Verify that the compressed file has the same contents as the original before replacing it
    ///
    /// This is an extra safety check to ensure that the compressed file is exactly the same as the
    /// original file.
    #[arg(long)]
    verify: bool,
}

#[derive(Debug, clap::Args)]
struct Compress {
    /// Paths to recursively compress
    #[arg(required = true)]
    paths: Vec<PathBuf>,

    /// The compression level to use
    #[arg(
        short, long,
        default_value_t = 5,
        value_parser = clap::value_parser!(u32).range(1..=9)
    )]
    level: u32,

    /// The minimum compression ratio
    ///
    /// Files will be skipped if they compress to a larger size than this ratio
    /// of the original size
    ///
    /// A value of 0.0 (or less) will skip all files
    /// A value of 1.0 will only skip files which cannot be compressed at all
    /// Values greater than 1.0 are valid, and will allow forcing compression to
    /// be used even if it results in a larger file
    #[arg(short = 'r', long, default_value_t = 0.95)]
    minimum_compression_ratio: f64,

    /// The type of compression to use
    #[arg(short, long, value_enum, default_value_t = Compression::default())]
    compression: Compression,

    /// Verify that the compressed file has the same contents as the original before replacing it
    ///
    /// This is an extra safety check to ensure that the compressed file is exactly the same as the
    /// original file.
    #[arg(long)]
    verify: bool,
}

#[derive(Debug, clap::Args)]
struct Info {
    /// Paths to inspect
    ///
    /// Info will be reported for each path
    #[arg(required = true)]
    paths: Vec<PathBuf>,
}

#[derive(Debug, Copy, Clone, clap::ValueEnum, PartialEq, Eq)]
enum Compression {
    #[cfg(feature = "lzfse")]
    Lzfse,
    #[cfg(feature = "zlib")]
    Zlib,
    #[cfg(feature = "lzvn")]
    Lzvn,
}

impl From<Compression> for compressor::Kind {
    fn from(c: Compression) -> Self {
        match c {
            #[cfg(feature = "zlib")]
            Compression::Zlib => compressor::Kind::Zlib,
            #[cfg(feature = "lzfse")]
            Compression::Lzfse => compressor::Kind::Lzfse,
            #[cfg(feature = "lzvn")]
            Compression::Lzvn => compressor::Kind::Lzvn,
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

    let writer = {
        cfg_if! {
            if #[cfg(feature = "zlib")] {
                flate2::write::GzEncoder::new(file, flate2::Compression::default())
            } else {
                file
            }
        }
    };
    Some(BufWriter::new(writer))
}

fn main() {
    let cli = Cli::parse();
    let verbosity = cli.verbosity();

    let mut _chrome_guard = None;
    let chrome_file = chrome_tracing_file(cli.chrome_tracing.as_deref());
    let chrome_layer: Option<_> = chrome_file.map(|f| {
        let (layer, guard) = ChromeLayerBuilder::new()
            .writer(f)
            .include_args(true)
            .build();
        _chrome_guard = Some(guard);
        layer
    });

    let progress_bars = ProgressBars::new(cli.verbosity());
    let fmt_writer = Mutex::new(LineWriter::new(ProgressBarWriter::new(
        progress_bars.multi_progress().clone(),
        std::io::stderr(),
    )));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_timer(time::uptime())
        .with_writer(fmt_writer)
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        );

    tracing_subscriber::registry()
        .with(chrome_layer)
        .with(fmt_layer)
        .init();

    match cli.command {
        Commands::Compress(Compress {
            paths,
            compression,
            minimum_compression_ratio,
            level,
            verify,
        }) => {
            let kind: Kind = compression.into();

            if kind != Kind::Zlib && level != 5 {
                tracing::warn!("Compression level is ignored for non-zlib compression");
            }

            let mut compressor = applesauce::FileCompressor::new();
            let stats = compressor.recursive_compress(
                paths.iter().map(Path::new),
                kind,
                minimum_compression_ratio,
                level,
                &progress_bars,
                verify,
            );
            progress_bars.finish();
            drop(progress_bars);
            tracing::info!("Finished compressing");
            if verbosity >= Verbosity::Normal {
                display_stats(&stats, true);
            }
        }
        Commands::Decompress(Decompress {
            paths,
            manual,
            verify,
        }) => {
            let mut compressor = applesauce::FileCompressor::new();
            let stats = compressor.recursive_decompress(
                paths.iter().map(Path::new),
                manual,
                &progress_bars,
                verify,
            );
            progress_bars.finish();
            tracing::info!("Finished decompressing");
            if verbosity >= Verbosity::Normal {
                display_stats(&stats, false);
            }
        }
        Commands::Info(info) => {
            for path in info.paths {
                if path.is_dir() {
                    let info = info::get_recursive(&path);
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
                    println!("\n{}:", path.display());

                    println!("Number of compressed files: {}", info.num_compressed_files);
                    println!("Total number of files: {}", info.num_files);
                    println!("Total number of folders: {}", info.num_folders);
                    println!(
                        "Total uncompressed size: {} ({})",
                        format_bytes(info.total_uncompressed_size),
                        info.total_uncompressed_size
                    );
                    println!(
                        "Total compressed size: {} ({})",
                        format_bytes(info.total_compressed_size),
                        info.total_compressed_size
                    );
                    println!(
                        "Compression Savings: {:.1}%",
                        info.compression_savings_fraction() * 100.0,
                    );
                } else {
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
                        );
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
}

pub fn display_stats(stats: &Stats, compress_mode: bool) {
    println!("Total Files: {}", stats.files.load(Ordering::Relaxed));
    let total_file_sizes = stats.total_file_sizes.load(Ordering::Relaxed);

    let compressed_count_start = stats.compressed_file_count_start.load(Ordering::Relaxed);
    let compressed_count_final = stats.compressed_file_count_final.load(Ordering::Relaxed);
    if compress_mode {
        println!(
            "New Files Compressed: {} ({} total compressed)",
            compressed_count_final.saturating_sub(compressed_count_start),
            compressed_count_final,
        );
    } else {
        print!(
            "Files Decompressed: {}",
            compressed_count_start.saturating_sub(compressed_count_final),
        );
        if compressed_count_final != 0 {
            println!(" ({} remaining compressed)", compressed_count_final);
        } else {
            println!();
        }
    }

    let compressed_size_start = stats.compressed_size_start.load(Ordering::Relaxed);
    let compressed_size_final = stats.compressed_size_final.load(Ordering::Relaxed);
    println!(
        "Starting Size (total filesize): {} ({})",
        format_bytes(total_file_sizes),
        total_file_sizes,
    );
    println!(
        "Starting Size (on disk):        {} ({})",
        format_bytes(compressed_size_start),
        compressed_size_start,
    );
    println!(
        "Final Size (on disk):           {} ({})",
        format_bytes(compressed_size_final),
        compressed_size_final,
    );
    println!(
        "Savings:                        {:.1}%",
        stats.compression_change_portion() * 100.0
    );
}

#[must_use]
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

fn format_bytes(byte_size: u64) -> impl fmt::Display {
    humansize::SizeFormatter::new(byte_size, humansize::BINARY)
}

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

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
