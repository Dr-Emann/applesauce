use applesauce_core::compressor::Compressor;
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn criterion_benchmark(c: &mut Criterion) {
    let data = black_box(random_data(8 * 1024));

    let mut group = c.benchmark_group("compress");
    group.throughput(criterion::Throughput::Bytes(data.len() as u64));

    #[cfg(feature = "lzfse")]
    group.bench_function("lzfse", |b| {
        let mut dst = vec![0; data.len() + 1024];
        let mut compressor = Compressor::lzfse();
        b.iter(|| {
            let res = compressor.compress(&mut dst, &data, 5).unwrap();

            black_box(&dst);

            res
        })
    });

    #[cfg(feature = "lzvn")]
    group.bench_function("lzvn", |b| {
        let mut dst = vec![0; data.len() + 1024];
        let mut compressor = Compressor::lzvn();
        b.iter(|| {
            let res = compressor.compress(&mut dst, &data, 5).unwrap();

            black_box(&dst);

            res
        })
    });

    #[cfg(feature = "zlib")]
    group.bench_function("zlib", |b| {
        let mut dst = vec![0; data.len() + 1024];
        let mut compressor = Compressor::lzvn();
        b.iter(|| {
            let res = compressor.compress(&mut dst, &data, 5).unwrap();

            black_box(&dst);

            res
        })
    });
    group.finish();
}

fn random_data(len: usize) -> Vec<u8> {
    let mut data = vec![0; len];
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
