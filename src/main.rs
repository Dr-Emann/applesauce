use applesauce::compressor::Compressor;
use cfg_if::cfg_if;
use clap::Parser;
use rayon::prelude::*;
use std::path::PathBuf;
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::fmt::time;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use walkdir::WalkDir;

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
    let (chrome_layer, _guard) = ChromeLayerBuilder::new()
        .file("/tmp/trace.json")
        .include_args(true)
        .build();
    let fmt_layer = tracing_subscriber::fmt::layer().with_timer(time::uptime());
    tracing_subscriber::registry()
        .with(chrome_layer)
        .with(fmt_layer)
        .init();

    let cli = {
        let _enter = tracing::debug_span!("cli parsing").entered();

        Cli::parse()
    };

    match cli.command {
        Commands::Compress(Compress { paths, compression }) => {
            paths
                .par_iter()
                .flat_map(|root| {
                    WalkDir::new(&root)
                        .into_iter()
                        .par_bridge()
                        .map(move |x| (x, root))
                })
                .for_each_init(
                    || applesauce::FileCompressor::new(compression.compressor()),
                    |compressor, (entry, root)| {
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

                        match compressor.compress_path(&full_path) {
                            Ok(()) => {
                                tracing::info!("successfully compressed {}", full_path.display())
                            }
                            Err(e) => {
                                tracing::error!("Error compressing {}: {}", full_path.display(), e)
                            }
                        }
                    },
                );
        }
    }
}
