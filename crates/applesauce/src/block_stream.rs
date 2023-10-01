use futures::prelude::*;
use std::cell::RefCell;
use std::fs::{File, Metadata};
use std::io::{self, Read, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::NamedTempFile;

use applesauce_core::compressor;
use applesauce_core::compressor::Kind;
use tokio::task::spawn_blocking;

struct ReadAtReader<'a> {
    file: &'a File,
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

pub fn chunked_stream(file: File, chunk_size: u64) -> impl Stream<Item = io::Result<Vec<u8>>> {
    // Read chunks with read_at
    let file = Arc::new(file);
    let stream = stream::iter(0..);
    let stream = stream
        .map(move |i| {
            let file = Arc::clone(&file);
            async move {
                let offset = i * chunk_size;

                spawn_blocking(move || -> io::Result<Vec<u8>> {
                    let mut data = Vec::with_capacity(chunk_size.try_into().unwrap());
                    let reader = ReadAtReader {
                        file: &file,
                        offset,
                    };
                    reader.take(chunk_size).read_to_end(&mut data)?;
                    Ok(data)
                })
                .await
                .unwrap()
            }
        })
        .buffered(10)
        .take_while(|res| future::ready(res.as_ref().map_or(true, |v| !v.is_empty())));
    stream
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

pub async fn compress(block: Vec<u8>, compressor_kind: compressor::Kind) -> Vec<u8> {
    let (tx, rx) = oneshot::channel();
    #[allow(clippy::uninit_vec)]
    rayon::spawn(move || {
        let mut result = Vec::with_capacity(applesauce_core::BLOCK_SIZE + 1024);
        unsafe { result.set_len(result.capacity()) };
        let res = with_compressor(compressor_kind, |compressor| {
            compressor.compress(&mut result, &block, 5)
        });
        unsafe { result.set_len(res.unwrap()) };
        let _ = tx.send(result);
    });
    rx.await.unwrap()
}

pub fn compress_stream(
    s: impl Stream<Item = io::Result<Vec<u8>>>,
) -> impl Stream<Item = io::Result<Vec<u8>>> {
    s.map(|data| async move {
        match data {
            Ok(data) => Ok(compress(data, Kind::Zlib).await),
            Err(e) => Err(e),
        }
    })
    .buffered(
        std::thread::available_parallelism()
            .map(|x| x.get())
            .unwrap_or(1),
    )
}

pub async fn compress_file(path: PathBuf, metadata: Metadata) -> io::Result<()> {
    let file = File::open(&path)?;
    let stream = chunked_stream(file, applesauce_core::BLOCK_SIZE as u64);
    let stream = compress_stream(stream);
    tokio::pin!(stream);

    let mut tmp_file = tmp_file_for(&path)?;

    while let Some(block) = stream.next().await {
        let block = block?;
        let (res, t) = spawn_blocking(move || {
            let res = tmp_file.write_all(&block);
            (res, tmp_file)
        })
        .await
        .unwrap();
        res?;
        tmp_file = t;
    }

    tmp_file.persist(path.with_extension("cmp"))?;

    Ok(())
}

fn tmp_file_for(path: &Path) -> io::Result<NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    if let Some(name) = path.file_name() {
        builder.prefix(name);
    }
    builder.tempfile_in(path.parent().ok_or(io::ErrorKind::InvalidInput)?)
}
