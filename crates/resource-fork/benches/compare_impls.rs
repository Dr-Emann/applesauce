use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use resource_fork::ResourceFork;
use std::fs::File;
use std::io::{Read, Write};
use tempfile::NamedTempFile;

fn criterion_benchmark(c: &mut Criterion) {
    let data = random_data();
    let mut g = c.benchmark_group("write");
    g.throughput(criterion::Throughput::Bytes(data.len() as u64));
    g.bench_function("rsrc_path", |b| {
        b.iter_batched_ref(
            || NamedTempFile::new().unwrap(),
            |file| {
                let mut rsrc_file = File::create(file.path().join("..namedfork/rsrc")).unwrap();
                rsrc_file.write_all(&data).unwrap();
                rsrc_file.flush().unwrap();
                drop(rsrc_file);
            },
            // Limit the number of open files
            BatchSize::LargeInput,
        );
    });
    g.bench_function("xattr", |b| {
        b.iter_batched_ref(
            || NamedTempFile::new().unwrap(),
            |file| {
                let mut rsrc_file = ResourceFork::new(file.as_file());
                rsrc_file.write_all(&data).unwrap();
                rsrc_file.flush().unwrap();
                drop(rsrc_file);
            },
            // Limit the number of open files
            BatchSize::LargeInput,
        );
    });
    g.finish();

    let mut output_data = vec![0; data.len()];
    let mut g = c.benchmark_group("read");
    g.throughput(criterion::Throughput::Bytes(data.len() as u64));
    g.bench_function("rsrc_path", |b| {
        b.iter_batched_ref(
            || {
                let file = NamedTempFile::new().unwrap();
                let mut rsrc_file = File::create(file.path().join("..namedfork/rsrc")).unwrap();
                rsrc_file.write_all(&data).unwrap();
                file
            },
            |file| {
                let mut rsrc_file = File::open(file.path().join("..namedfork/rsrc")).unwrap();
                rsrc_file.read_exact(&mut output_data).unwrap();
            },
            // Limit the number of open files
            BatchSize::LargeInput,
        );
    });
    g.bench_function("xattr", |b| {
        b.iter_batched_ref(
            || {
                let file = NamedTempFile::new().unwrap();
                let mut rsrc_file = ResourceFork::new(file.as_file());
                rsrc_file.write_all(&data).unwrap();
                file
            },
            |file| {
                let mut rsrc_file = ResourceFork::new(file.as_file());
                rsrc_file.read_exact(&mut output_data).unwrap();
            },
            // Limit the number of open files
            BatchSize::LargeInput,
        );
    });
    g.finish();
}

fn random_data() -> Vec<u8> {
    let mut data = vec![0; 8 * 1024 * 1024];
    let mut rng = Xorshift32 { state: 0x193a6754 };
    rng.fill_bytes(&mut data);
    data
}

struct Xorshift32 {
    state: u32,
}

impl Xorshift32 {
    fn next(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }

    fn fill_bytes(&mut self, buf: &mut [u8]) {
        let mut iter = buf.chunks_exact_mut(4);
        for chunk in &mut iter {
            let value = self.next();
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        let remainder = iter.into_remainder();
        if remainder.is_empty() {
            return;
        }
        remainder.copy_from_slice(&self.next().to_le_bytes()[..remainder.len()]);
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
