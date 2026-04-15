#![allow(dead_code)]

#[path = "../tests/support/mod.rs"]
mod support;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use paradown::download::{DownloadSpec, Manager};
use paradown::{Backend, Config};
use support::{MultiFileServerConfig, MultiFileTestServer, TestAsset};
use tempfile::tempdir;

fn download_benchmark(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let body = vec![0x5Au8; 256 * 1024];

    c.bench_function("http_single_asset_download", |bench| {
        bench.to_async(&runtime).iter_batched(
            || body.clone(),
            |payload| async move {
                let server = MultiFileTestServer::spawn(
                    vec![TestAsset::from_shared(
                        "/bench.bin",
                        std::sync::Arc::new(payload),
                    )],
                    MultiFileServerConfig::default(),
                )
                .await;
                let temp = tempdir().expect("tempdir");

                let mut config = Config::default();
                config.download_dir = temp.path().join("downloads");
                config.storage_backend = Backend::Memory;
                config.segments_per_task = 4;
                config.concurrent_tasks = 1;

                let manager = Manager::new(config).expect("manager");
                manager.init().await.expect("init");

                let task_id = manager
                    .add_download(DownloadSpec::parse(server.url("/bench.bin")).expect("spec"))
                    .await
                    .expect("add_download");
                manager.start_task(task_id).await.expect("start");
                manager.wait_for_all_tasks().await.expect("wait");
            },
            BatchSize::SmallInput,
        );
    });
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
