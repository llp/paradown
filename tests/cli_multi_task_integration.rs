mod support;

use std::process::{Command, Stdio};
use std::sync::Arc;
use support::{MultiFileServerConfig, MultiFileTestServer, TestAsset};
use tempfile::TempDir;
use tokio::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_process_handles_multi_task_downloads_end_to_end() {
    let alpha = Arc::new(
        (0..(96 * 1024))
            .map(|idx| (idx % 233) as u8)
            .collect::<Vec<_>>(),
    );
    let beta = Arc::new(
        (0..(84 * 1024))
            .map(|idx| ((idx * 7) % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let server = MultiFileTestServer::spawn(
        vec![
            TestAsset::from_shared("/alpha.bin", Arc::clone(&alpha)),
            TestAsset::from_shared("/beta.bin", Arc::clone(&beta)),
        ],
        MultiFileServerConfig {
            response_chunk_size: 4 * 1024,
            chunk_delay: Duration::from_millis(10),
        },
    )
    .await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let stdout_path = sandbox.path().join("stdout.log");
    let stderr_path = sandbox.path().join("stderr.log");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let stdout_file = std::fs::File::create(&stdout_path).unwrap();
    let stderr_file = std::fs::File::create(&stderr_path).unwrap();

    let binary = env!("CARGO_BIN_EXE_paradown").to_string();
    let sandbox_dir = sandbox.path().to_path_buf();
    let alpha_url = server.url("/alpha.bin");
    let beta_url = server.url("/beta.bin");
    let download_dir_for_cmd = download_dir.clone();
    let status = tokio::task::spawn_blocking(move || {
        Command::new(binary)
            .current_dir(sandbox_dir)
            .arg("--download-dir")
            .arg(download_dir_for_cmd)
            .arg("--verbose")
            .arg("--max-concurrent")
            .arg("2")
            .arg("--workers")
            .arg("3")
            .arg("--urls")
            .arg(alpha_url)
            .arg(beta_url)
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file))
            .status()
    })
    .await
    .unwrap()
    .unwrap();

    let stdout = tokio::fs::read_to_string(&stdout_path).await.unwrap();
    let stderr = tokio::fs::read_to_string(&stderr_path).await.unwrap();
    assert!(
        status.success(),
        "cli exited with status {:?}\nstdout:\n{}\nstderr:\n{}",
        status.code(),
        stdout,
        stderr
    );

    assert!(
        stdout.contains("Final summary: tasks=2 completed=2 failed=0 canceled=0 deleted=0"),
        "expected final summary in CLI output\nstdout:\n{}",
        stdout
    );
    assert!(
        stderr.contains("Task #1 started") && stderr.contains("Task #2 started"),
        "expected verbose task start logs\nstderr:\n{}",
        stderr
    );

    let alpha_downloaded = tokio::fs::read(download_dir.join("alpha.bin"))
        .await
        .unwrap();
    let beta_downloaded = tokio::fs::read(download_dir.join("beta.bin"))
        .await
        .unwrap();
    assert_eq!(alpha_downloaded, *alpha);
    assert_eq!(beta_downloaded, *beta);

    assert!(
        server.max_active_transfers() >= 2,
        "expected overlapping transfers during CLI run"
    );
    assert!(
        server.range_get_count("/alpha.bin").await >= 2,
        "expected multi-segment ranged GETs for alpha.bin"
    );
    assert!(
        server.range_get_count("/beta.bin").await >= 2,
        "expected multi-segment ranged GETs for beta.bin"
    );
}
