use criterion::{criterion_group, criterion_main, Criterion};

fn download_benchmark(c: &mut Criterion) {

}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .significance_level(0.1)
        .noise_threshold(0.05)
        .configure_from_args();
    targets = download_benchmark
}
criterion_main!(benches);