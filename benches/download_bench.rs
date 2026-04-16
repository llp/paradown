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

    c.bench_function("http_single_asset_download_memory", |bench| {
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

    c.bench_function("http_multi_task_download_sqlite", |bench| {
        bench.to_async(&runtime).iter_batched(
            || (vec![0x3Cu8; 192 * 1024], vec![0x7Du8; 224 * 1024]),
            |(alpha, beta)| async move {
                let server = MultiFileTestServer::spawn(
                    vec![
                        TestAsset::from_shared("/alpha.bin", std::sync::Arc::new(alpha)),
                        TestAsset::from_shared("/beta.bin", std::sync::Arc::new(beta)),
                    ],
                    MultiFileServerConfig::default(),
                )
                .await;
                let temp = tempdir().expect("tempdir");

                let mut config = Config::default();
                config.download_dir = temp.path().join("downloads");
                config.storage_backend = Backend::Sqlite(temp.path().join("state.db"));
                config.segments_per_task = 4;
                config.concurrent_tasks = 2;
                config.http.client.proxy.use_env_proxy = false;

                let manager = Manager::new(config).expect("manager");
                manager.init().await.expect("init");

                let alpha_id = manager
                    .add_download(
                        DownloadSpec::parse(server.url("/alpha.bin")).expect("alpha spec"),
                    )
                    .await
                    .expect("add alpha");
                let beta_id = manager
                    .add_download(DownloadSpec::parse(server.url("/beta.bin")).expect("beta spec"))
                    .await
                    .expect("add beta");

                manager.start_task(alpha_id).await.expect("start alpha");
                manager.start_task(beta_id).await.expect("start beta");
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
