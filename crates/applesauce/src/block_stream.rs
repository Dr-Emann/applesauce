use futures::prelude::*;
use std::cell::RefCell;
use std::fs::Metadata;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::atomic::AtomicU64;
use std::sync::{atomic, Arc};
use tempfile::NamedTempFile;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

use applesauce_core::{compressor, num_blocks};
use tokio::task::spawn_blocking;
use tracing::{info_span, Span};

struct ReadAtReader<'a> {
    file: &'a std::fs::File,
    offset: u64,
}

impl<'a> io::Read for ReadAtReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read_len = self.file.read_at(buf, self.offset);
        if let Ok(len) = read_len {
            self.offset += len as u64;
        }
        read_len
    }
}

struct BlockLimiter {
    max_blocks: u64,
    current_blocks: AtomicU64,
    notify: Notify,
}

impl BlockLimiter {
    pub fn new(max_blocks: u64) -> Self {
        assert!(max_blocks > 0, "max_blocks must be > 0");
        Self {
            max_blocks,
            current_blocks: AtomicU64::new(0),
            notify: Notify::new(),
        }
    }

    fn return_block(&self) {
        let prev_val = self.current_blocks.fetch_sub(1, atomic::Ordering::Relaxed);
        debug_assert_ne!(prev_val, 0);
        let new_val = prev_val - 1;
        if new_val < self.max_blocks {
            self.notify.notify_one();
        }
    }

    fn return_blocks(&self, count: u64) {
        if count == 0 {
            return;
        }
        let prev_val = self
            .current_blocks
            .fetch_sub(count, atomic::Ordering::Relaxed);
        debug_assert!(prev_val >= count);
        self.notify.notify_waiters();
    }

    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn acquire(self: &Arc<Self>, blocks: u64) -> OutstandingBlocks {
        let mut notified = pin!(self.notify.notified());
        loop {
            notified.as_mut().enable();
            let mut current_blocks = self.current_blocks.load(atomic::Ordering::Relaxed);
            while current_blocks < self.max_blocks {
                let new_blocks = current_blocks
                    .checked_add(blocks)
                    .expect("overflow on block count");
                let exchange_res = self.current_blocks.compare_exchange_weak(
                    current_blocks,
                    new_blocks,
                    atomic::Ordering::Relaxed,
                    atomic::Ordering::Relaxed,
                );
                match exchange_res {
                    Ok(_) => {
                        return OutstandingBlocks {
                            block_limiter: Arc::clone(self),
                            count: blocks,
                        };
                    }
                    Err(n) => current_blocks = n,
                }
            }
            notified.as_mut().await;
            // Reset the future in case another call to acquire got the message before us
            notified.set(self.notify.notified());
        }
    }
}

struct OutstandingBlocks {
    block_limiter: Arc<BlockLimiter>,
    count: u64,
}

impl OutstandingBlocks {
    pub fn return_block(&mut self) {
        self.count = match self.count.checked_sub(1) {
            Some(c) => c,
            None => return,
        };
        self.block_limiter.return_block();
    }
}

struct InputBlock {
    index: u64,
    data: Vec<u8>,
    #[allow(dead_code)]
    permit: OwnedSemaphorePermit,
}

impl Drop for OutstandingBlocks {
    fn drop(&mut self) {
        self.block_limiter.return_blocks(self.count);
    }
}

struct StreamCompressor {
    pool: rayon::ThreadPool,
    block_limit: Arc<BlockLimiter>,
    sem: Arc<Semaphore>,
}

impl StreamCompressor {
    fn new() -> Self {
        let pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("stream-compressor-worker-{i}"))
            .build()
            .unwrap();
        let target_blocks = pool.current_num_threads() * 2;
        let sem = Arc::new(Semaphore::new(target_blocks));
        Self {
            pool,
            sem,
            block_limit: Arc::new(BlockLimiter::new(target_blocks as u64)),
        }
    }

    pub fn chunked_stream(
        &self,
        file: std::fs::File,
        chunk_size: u64,
    ) -> impl Stream<Item = io::Result<InputBlock>> + '_ {
        // Read chunks with read_at
        let file = Arc::new(file);
        stream::iter(0..)
            .map(move |i| {
                let file = Arc::clone(&file);
                let offset = i * chunk_size;
                let parent_span = Span::current();
                async move {
                    let permit = self.get_permit().await?;
                    spawn_blocking(move || -> io::Result<InputBlock> {
                        let _span = info_span!(parent: parent_span, "reading block", i).entered();
                        use std::io::prelude::*;

                        let mut data = Vec::with_capacity(chunk_size.try_into().unwrap());
                        let reader = ReadAtReader {
                            file: &file,
                            offset,
                        };
                        reader.take(chunk_size).read_to_end(&mut data)?;
                        Ok(InputBlock {
                            index: i,
                            data,
                            permit,
                        })
                    })
                    .await
                    .unwrap()
                }
            })
            .buffered(100)
            .try_take_while(|block| future::ready(Ok(!block.data.is_empty())))
    }

    async fn compress_file(
        &self,
        path: PathBuf,
        metadata: Metadata,
    ) -> impl Future<Output = io::Result<()>> + '_ {
        let blocks = num_blocks(metadata.len());
        let outstanding_blocks = self.block_limit.acquire(blocks).await;

        self._compress_file(path, metadata, outstanding_blocks)
    }

    #[tracing::instrument(skip(self, path, outstanding_blocks))]
    async fn _compress_file(
        &self,
        path: PathBuf,
        metadata: Metadata,
        outstanding_blocks: OutstandingBlocks,
    ) -> io::Result<()> {
        let path: Arc<Path> = Arc::from(path);
        let file = File::open(&path).await?;
        let (tx, rx) = tokio::sync::mpsc::channel(1);

        let write_handle = async {
            tokio::spawn(write_file(
                tokio_stream::wrappers::ReceiverStream::new(rx),
                path,
                outstanding_blocks,
            ))
            .await
            .expect("write_file task panicked")
        };

        let stream = self.chunked_stream(file.into_std().await, applesauce_core::BLOCK_SIZE as u64);
        let stream = self.compress_stream(stream);
        let forward_task = async {
            let mut stream = pin!(stream);
            while let Some(item) = stream.try_next().await? {
                if tx.send(item).await.is_err() {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
            }
            drop(tx);
            Ok(())
        };

        ((), ()) = tokio::try_join!(forward_task, write_handle)?;

        Ok(())
    }

    async fn get_permit(&self) -> io::Result<OwnedSemaphorePermit> {
        Arc::clone(&self.sem)
            .acquire_owned()
            .await
            .map_err(|_| io::ErrorKind::BrokenPipe.into())
    }

    async fn compress(
        &self,
        block: InputBlock,
        compressor_kind: compressor::Kind,
    ) -> io::Result<Vec<u8>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let parent_span = Span::current();
        self.pool.spawn(move || {
            // Move into thread
            let _span = tracing::info_span!(parent: parent_span, "compress_block", i = block.index)
                .entered();
            let mut result = Vec::with_capacity(block.data.len() + 1024);
            let res = with_compressor(compressor_kind, |compressor| {
                compressor.compress(
                    unsafe {
                        std::slice::from_raw_parts_mut(result.as_mut_ptr(), result.capacity())
                    },
                    &block.data,
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
        rx.await.expect("compressor thread panicked")
    }

    pub fn compress_stream<'a>(
        &'a self,
        s: impl Stream<Item = io::Result<InputBlock>> + 'a,
    ) -> impl Stream<Item = io::Result<Vec<u8>>> + 'a {
        s.map_ok(move |block| self.compress(block, compressor::Kind::Lzfse))
            .try_buffered(64)
    }
}

#[tracing::instrument(skip(stream, path, outstanding_blocks))]
async fn write_file(
    stream: impl Stream<Item = Vec<u8>>,
    path: Arc<Path>,
    mut outstanding_blocks: OutstandingBlocks,
) -> io::Result<()> {
    let mut tmp_file = tmp_file_for(Arc::clone(&path)).await?;
    let mut stream = pin!(stream);

    while let Some(block) = stream.next().await {
        tmp_file.as_file_mut().write_all(&block).await?;
        outstanding_blocks.return_block();
    }

    spawn_blocking(move || tmp_file.persist(path.with_extension("cmp")))
        .await
        .unwrap()?;

    Ok(())
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

pub async fn compress_file(path: PathBuf, metadata: Metadata) -> io::Result<()> {
    let compressor = StreamCompressor::new();
    info_span!("setting up pool").in_scope(|| {
        compressor.pool.broadcast(|_| {
            COMPRESSORS.with(|compressors| {
                let mut compressors = compressors.borrow_mut();
                let idx = compressor::Kind::Lzfse as usize;
                if idx >= compressors.len() {
                    compressors.resize_with(idx + 1, || None);
                }
                let _ = compressors[idx]
                    .get_or_insert_with(|| compressor::Kind::Lzfse.compressor().unwrap());
            });
        });
    });
    compressor.compress_file(path, metadata).flatten().await
}

#[tracing::instrument(level = "debug")]
async fn tmp_file_for(path: Arc<Path>) -> io::Result<NamedTempFile<File>> {
    let mut builder = tempfile::Builder::new();
    if let Some(name) = path.file_name() {
        builder.prefix(name);
    }
    spawn_blocking(move || {
        let mut builder = tempfile::Builder::new();
        if let Some(name) = path.file_name() {
            builder.prefix(name);
        }
        let named_std_file =
            builder.tempfile_in(path.parent().ok_or(io::ErrorKind::InvalidInput)?)?;
        let (std_file, path) = named_std_file.into_parts();
        Ok(NamedTempFile::from_parts(File::from_std(std_file), path))
    })
    .await
    .expect("panic in spawned task")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn test_limiter() {
        let limiter = Arc::new(BlockLimiter::new(2));
        let outstanding = limiter.acquire(1).await;
        assert_eq!(limiter.current_blocks.load(atomic::Ordering::Relaxed), 1);
        drop(outstanding);
        assert_eq!(limiter.current_blocks.load(atomic::Ordering::Relaxed), 0);

        let mut outstanding = limiter.acquire(5).await;
        assert!(limiter.acquire(5).now_or_never().is_none());
        for _ in 0..3 {
            outstanding.return_block();
        }
        assert!(limiter.acquire(5).now_or_never().is_none());
        outstanding.return_block();
        assert!(limiter.acquire(5).now_or_never().is_some());
        let mut outstanding2 = limiter.acquire(5).await;
        assert_eq!(
            limiter.current_blocks.load(atomic::Ordering::Relaxed),
            outstanding.count + outstanding2.count
        );

        let running_task = {
            let limiter = Arc::clone(&limiter);
            tokio::spawn(async move {
                let _outstanding = tokio::join!(limiter.acquire(1), limiter.acquire(1));
            })
        };
        for _ in 0..5 {
            outstanding2.return_block();
        }
        drop(outstanding);
        tokio::time::timeout(Duration::from_millis(1), running_task)
            .await
            .unwrap()
            .unwrap();
    }
}
