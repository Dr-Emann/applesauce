use futures::prelude::*;
use std::cell::RefCell;
use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::Semaphore;

use applesauce_core::compressor;
use tokio::task::spawn_blocking;
use tracing::{Instrument, Span};

pub fn chunked_stream(file: File, chunk_size: u64) -> impl Stream<Item = io::Result<Vec<u8>>> {
    stream::try_unfold(file, move |mut file| {
        async move {
            let mut buf = Vec::with_capacity(chunk_size as usize);
            let n = (&mut file).take(chunk_size).read_to_end(&mut buf).await?;
            Ok((n != 0).then_some((buf, file)))
        }
        .instrument(tracing::info_span!("read_chunk"))
    })
}

struct StreamCompressor {
    pool: rayon::ThreadPool,
    sem: Arc<Semaphore>,
}

impl StreamCompressor {
    fn new() -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("stream-compressor-worker-{i}"))
            .build()
            .unwrap();
        let sem = Arc::new(Semaphore::new(pool.current_num_threads() * 2));
        Self { pool, sem }
    }

    async fn compress(
        &self,
        block: Vec<u8>,
        compressor_kind: compressor::Kind,
    ) -> io::Result<impl Future<Output = io::Result<Vec<u8>>>> {
        let _permit = Arc::clone(&self.sem)
            .acquire_owned()
            .await
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let parent_span = Span::current();
        self.pool.spawn(move || {
            // Move into thread
            let _permit = _permit;
            let _span = tracing::info_span!(parent: parent_span, "compress_block").entered();
            let mut result = Vec::with_capacity(block.len() + 1024);
            let res = with_compressor(compressor_kind, |compressor| {
                compressor.compress(
                    unsafe {
                        std::slice::from_raw_parts_mut(result.as_mut_ptr(), result.capacity())
                    },
                    &block,
                    5,
                )
            });
            match res {
                Ok(n) => {
                    unsafe { result.set_len(n) };
                    let _ = tx.send(Ok(result));
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        });
        Ok(async move { rx.await.unwrap() })
    }

    pub fn compress_stream<'a>(
        &'a self,
        s: impl TryStream<Ok = Vec<u8>, Error = io::Error> + 'a,
    ) -> impl Stream<Item = io::Result<Vec<u8>>> + 'a {
        s.and_then(move |block| self.compress(block, compressor::Kind::Zlib))
            .try_buffered(self.pool.current_num_threads() * 2)
    }
}

thread_local! {
    static COMPRESSORS: RefCell<Vec<Option<compressor::Compressor>>> = const { RefCell::new(Vec::new()) };
}

fn with_compressor<F, O>(compressor_kind: compressor::Kind, f: F) -> O
where
    F: FnOnce(&mut compressor::Compressor) -> O,
{
    COMPRESSORS.with(|compressors| {
        let mut compressors = compressors.borrow_mut();
        let idx = compressor_kind as usize;
        if idx >= compressors.len() {
            compressors.resize_with(idx + 1, || None);
        }
        let compressor =
            compressors[idx].get_or_insert_with(|| compressor_kind.compressor().unwrap());
        f(compressor)
    })
}

#[tracing::instrument(skip_all)]
pub async fn compress_file(path: PathBuf, metadata: Metadata) -> io::Result<()> {
    let file = File::open(&path).await?;
    let stream = chunked_stream(file, applesauce_core::BLOCK_SIZE as u64);
    let compressor = StreamCompressor::new();
    let stream = compressor.compress_stream(stream);
    tokio::pin!(stream);

    let mut tmp_file = tmp_file_for(&path)?;

    while let Some(block) = stream.try_next().await? {
        let (res, t) = spawn_blocking(move || {
            use std::io::Write;

            let res = tmp_file.write_all(&block);
            (res, tmp_file)
        })
        .await
        .unwrap();
        res?;
        tmp_file = t;
    }

    spawn_blocking(move || tmp_file.persist(path.with_extension("cmp")))
        .await
        .unwrap()?;

    Ok(())
}

fn tmp_file_for(path: &Path) -> io::Result<NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    if let Some(name) = path.file_name() {
        builder.prefix(name);
    }
    builder.tempfile_in(path.parent().ok_or(io::ErrorKind::InvalidInput)?)
}
