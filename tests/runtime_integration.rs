mod support;

use paradown::FileConflictStrategy;
use paradown::download::{DownloadSpec, Event, Manager, SessionRequest};
use paradown::repository::models::{DBDownloadTask, DBDownloadWorker};
use paradown::{
    Backend, Config, ConfigBuilder, SourceCapabilities, SourceDescriptor, SourceKind, SourceSet,
    Store,
};
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;
use support::{MultiFileServerConfig, MultiFileTestServer, TestAsset};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::time::{Duration, Instant, timeout};

#[tokio::test]
async fn restores_paused_task_from_sqlite_on_manager_init() {
    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let partial_file = download_dir.join("archive.bin");
    tokio::fs::write(&partial_file, vec![b'a'; 60])
        .await
        .unwrap();

    let config = build_config(
        &download_dir,
        &db_path,
        2,
        None,
        FileConflictStrategy::Resume,
    );
    let persistence = Store::new(Arc::new(config.clone())).await.unwrap();
    let repository = persistence.repository();

    repository
        .save_task(&DBDownloadTask {
            id: 7,
            url: "https://example.com/archive.bin".into(),
            spec_json: "".into(),
            source_set_json: "".into(),
            resolved_url: "https://example.com/archive.bin".into(),
            entity_tag: "\"etag-a\"".into(),
            last_modified: "Tue, 15 Apr 2026 12:00:00 GMT".into(),
            file_name: "archive.bin".into(),
            file_path: partial_file.to_string_lossy().to_string(),
            status: "Paused".into(),
            downloaded_size: 1,
            total_size: Some(100),
            created_at: None,
            updated_at: None,
        })
        .await
        .unwrap();
    repository
        .save_worker(&DBDownloadWorker {
            id: 1,
            task_id: 7,
            index: 0,
            source_id: Some("https::https://example.com/archive.bin".into()),
            piece_start: None,
            piece_end: None,
            block_start: None,
            block_end: None,
            start: 0,
            end: 49,
            downloaded: 50,
            status: "Completed".into(),
            updated_at: None,
        })
        .await
        .unwrap();
    repository
        .save_worker(&DBDownloadWorker {
            id: 2,
            task_id: 7,
            index: 1,
            source_id: Some("https::https://example.com/archive.bin".into()),
            piece_start: None,
            piece_end: None,
            block_start: None,
            block_end: None,
            start: 50,
            end: 99,
            downloaded: 10,
            status: "Running".into(),
            updated_at: None,
        })
        .await
        .unwrap();

    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let snapshot = manager
        .get_session_by_id(7)
        .expect("restored session should exist")
        .snapshot()
        .await;
    assert_eq!(snapshot.status, "Paused");
    assert_eq!(snapshot.downloaded_size, 60);
}

#[tokio::test]
async fn downloads_file_end_to_end_via_manager() {
    let body = Arc::new(
        (0..(64 * 1024))
            .map(|idx| (idx % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let server = TestDownloadServer::spawn(Arc::clone(&body), 8 * 1024).await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = build_config(
        &download_dir,
        &db_path,
        4,
        None,
        FileConflictStrategy::Overwrite,
    );
    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let mut rx = manager.subscribe_events();
    let task_id = manager
        .add_download(DownloadSpec::parse(server.url("/file.bin")).unwrap())
        .await
        .unwrap();
    manager.start_task(task_id).await.unwrap();

    wait_for_task_completion(task_id, &mut rx).await.unwrap();

    let downloaded = tokio::fs::read(download_dir.join("file.bin"))
        .await
        .unwrap();
    assert_eq!(downloaded, *body);

    let snapshot = manager.get_session_by_id(task_id).unwrap().snapshot().await;
    assert_eq!(snapshot.status, "Completed");
}

#[tokio::test]
async fn respects_global_rate_limit_during_download() {
    let body = Arc::new(vec![b'x'; 64 * 1024]);
    let server = TestDownloadServer::spawn(Arc::clone(&body), body.len()).await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = build_config(
        &download_dir,
        &db_path,
        1,
        NonZeroU64::new(32),
        FileConflictStrategy::Overwrite,
    );
    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let mut rx = manager.subscribe_events();
    let task_id = manager
        .add_download(DownloadSpec::parse(server.url("/limited.bin")).unwrap())
        .await
        .unwrap();

    let started_at = Instant::now();
    manager.start_task(task_id).await.unwrap();
    wait_for_task_completion(task_id, &mut rx).await.unwrap();

    assert!(
        started_at.elapsed() >= Duration::from_millis(1500),
        "rate limit should noticeably slow the download"
    );
}

#[tokio::test]
async fn downloads_multiple_tasks_concurrently_and_restores_completed_state() {
    let alpha = Arc::new(
        (0..(96 * 1024))
            .map(|idx| (idx % 239) as u8)
            .collect::<Vec<_>>(),
    );
    let beta = Arc::new(
        (0..(88 * 1024))
            .map(|idx| ((idx * 3) % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let server = MultiFileTestServer::spawn(
        vec![
            TestAsset::from_shared("/alpha.bin", Arc::clone(&alpha)),
            TestAsset::from_shared("/beta.bin", Arc::clone(&beta)),
        ],
        MultiFileServerConfig {
            response_chunk_size: 4 * 1024,
            chunk_delay: Duration::from_millis(8),
        },
    )
    .await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = build_config_with_concurrency(
        &download_dir,
        &db_path,
        4,
        2,
        None,
        FileConflictStrategy::Overwrite,
    );
    let manager = Manager::new(config.clone()).unwrap();
    manager.init().await.unwrap();

    let mut rx = manager.subscribe_events();
    let alpha_task = manager
        .add_download(DownloadSpec::parse(server.url("/alpha.bin")).unwrap())
        .await
        .unwrap();
    let beta_task = manager
        .add_download(DownloadSpec::parse(server.url("/beta.bin")).unwrap())
        .await
        .unwrap();

    manager.start_task(alpha_task).await.unwrap();
    manager.start_task(beta_task).await.unwrap();

    wait_for_task_set_completion(&[alpha_task, beta_task], &mut rx)
        .await
        .unwrap();

    let alpha_downloaded = tokio::fs::read(download_dir.join("alpha.bin"))
        .await
        .unwrap();
    let beta_downloaded = tokio::fs::read(download_dir.join("beta.bin"))
        .await
        .unwrap();
    assert_eq!(alpha_downloaded, *alpha);
    assert_eq!(beta_downloaded, *beta);

    let alpha_snapshot = manager
        .get_session_by_id(alpha_task)
        .unwrap()
        .snapshot()
        .await;
    assert_eq!(alpha_snapshot.status, "Completed");
    assert_eq!(alpha_snapshot.completed_pieces, alpha_snapshot.piece_count);

    let beta_snapshot = manager
        .get_session_by_id(beta_task)
        .unwrap()
        .snapshot()
        .await;
    assert_eq!(beta_snapshot.status, "Completed");
    assert_eq!(beta_snapshot.completed_pieces, beta_snapshot.piece_count);

    assert!(
        server.max_active_transfers() >= 2,
        "expected overlapping download transfers across multiple tasks"
    );
    assert!(
        server.range_get_count("/alpha.bin").await >= 2,
        "expected multiple ranged GETs for alpha.bin"
    );
    assert!(
        server.range_get_count("/beta.bin").await >= 2,
        "expected multiple ranged GETs for beta.bin"
    );

    drop(manager);

    let persistence = Store::new(Arc::new(config.clone())).await.unwrap();
    let mut bundles = persistence.load_task_bundles().await.unwrap();
    bundles.sort_by_key(|bundle| bundle.task.id);
    assert_eq!(bundles.len(), 2);
    for bundle in &bundles {
        assert_eq!(bundle.task.status, "Completed");
        assert_eq!(bundle.task.downloaded_size, bundle.task.total_size.unwrap());
        assert!(bundle.workers.len() >= 2);
        assert!(!bundle.pieces.is_empty());
        assert!(bundle.pieces.iter().all(|piece| piece.completed));
    }

    let restored_manager = Manager::new(config).unwrap();
    restored_manager.init().await.unwrap();
    let mut restored_sessions = restored_manager.get_all_sessions();
    restored_sessions.sort_by_key(|session| session.id());
    assert_eq!(restored_sessions.len(), 2);
    for session in restored_sessions {
        let snapshot = session.snapshot().await;
        assert_eq!(snapshot.status, "Completed");
        assert_eq!(snapshot.completed_pieces, snapshot.piece_count);
    }
}

#[tokio::test]
async fn deleting_a_running_session_removes_it_from_manager_and_unblocks_the_queue() {
    let alpha = Arc::new(
        (0..(256 * 1024))
            .map(|idx| (idx % 223) as u8)
            .collect::<Vec<_>>(),
    );
    let beta = Arc::new(
        (0..(72 * 1024))
            .map(|idx| ((idx * 11) % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let server = MultiFileTestServer::spawn(
        vec![
            TestAsset::from_shared("/alpha.bin", Arc::clone(&alpha)),
            TestAsset::from_shared("/beta.bin", Arc::clone(&beta)),
        ],
        MultiFileServerConfig {
            response_chunk_size: 4 * 1024,
            chunk_delay: Duration::from_millis(15),
        },
    )
    .await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = build_config_with_concurrency(
        &download_dir,
        &db_path,
        4,
        1,
        None,
        FileConflictStrategy::Overwrite,
    );
    let persistence = Store::new(Arc::new(config.clone())).await.unwrap();
    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let mut rx = manager.subscribe_events();
    let alpha_task = manager
        .add_download(DownloadSpec::parse(server.url("/alpha.bin")).unwrap())
        .await
        .unwrap();
    let beta_task = manager
        .add_download(DownloadSpec::parse(server.url("/beta.bin")).unwrap())
        .await
        .unwrap();

    manager.start_task(alpha_task).await.unwrap();
    manager.start_task(beta_task).await.unwrap();

    tokio::time::sleep(Duration::from_millis(120)).await;
    let alpha_session = manager
        .get_session_by_id(alpha_task)
        .expect("running session should still be registered before deletion");
    alpha_session.delete().await.unwrap();

    assert!(
        manager.get_session_by_id(alpha_task).is_none(),
        "deleted session should be removed from manager state",
    );

    wait_for_task_completion(beta_task, &mut rx).await.unwrap();

    let beta_downloaded = tokio::fs::read(download_dir.join("beta.bin"))
        .await
        .unwrap();
    assert_eq!(beta_downloaded, *beta);
    assert!(
        !download_dir.join("alpha.bin").exists(),
        "deleted task payload should be removed from disk",
    );
    assert!(persistence.load_task(alpha_task).await.unwrap().is_none());
    assert!(
        persistence
            .load_workers(alpha_task)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        persistence
            .load_pieces(alpha_task)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        persistence
            .load_blocks(alpha_task)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn distributes_http_segments_across_multiple_sources() {
    let body = Arc::new(
        (0..(128 * 1024))
            .map(|idx| ((idx * 7) % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let primary_server = MultiFileTestServer::spawn(
        vec![TestAsset::from_shared("/shared.bin", Arc::clone(&body))],
        MultiFileServerConfig {
            response_chunk_size: 2 * 1024,
            chunk_delay: Duration::from_millis(6),
        },
    )
    .await;
    let mirror_server = MultiFileTestServer::spawn(
        vec![TestAsset::from_shared("/shared.bin", Arc::clone(&body))],
        MultiFileServerConfig {
            response_chunk_size: 2 * 1024,
            chunk_delay: Duration::from_millis(6),
        },
    )
    .await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = build_config(
        &download_dir,
        &db_path,
        4,
        None,
        FileConflictStrategy::Overwrite,
    );
    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let primary_spec = DownloadSpec::parse(primary_server.url("/shared.bin")).unwrap();
    let mirror_spec = DownloadSpec::parse(mirror_server.url("/shared.bin")).unwrap();
    let mut sources = SourceSet::single_primary(SourceDescriptor::from_spec(&primary_spec, None));
    sources.sources.push(SourceDescriptor {
        id: format!("mirror::{}", mirror_spec.locator()),
        kind: SourceKind::Mirror,
        locator: mirror_spec.locator().to_string(),
        metadata_only: false,
        request: None,
        resource_identity: None,
        capabilities: SourceCapabilities::http_origin(),
    });

    let mut rx = manager.subscribe_events();
    let task_id = manager
        .add_session(
            SessionRequest::builder(primary_spec)
                .sources(sources)
                .build(),
        )
        .await
        .unwrap();
    manager.start_task(task_id).await.unwrap();
    wait_for_task_completion(task_id, &mut rx).await.unwrap();

    let downloaded = tokio::fs::read(download_dir.join("shared.bin"))
        .await
        .unwrap();
    assert_eq!(downloaded, *body);

    let snapshot = manager.get_session_by_id(task_id).unwrap().snapshot().await;
    assert_eq!(snapshot.status, "Completed");
    assert_eq!(snapshot.source_count, 2);
    assert!(
        snapshot
            .source_locators
            .iter()
            .any(|locator| locator == mirror_spec.locator())
    );

    assert!(
        primary_server.range_get_count("/shared.bin").await >= 1,
        "primary source should serve at least one ranged GET",
    );
    assert!(
        mirror_server.range_get_count("/shared.bin").await >= 1,
        "mirror source should serve at least one ranged GET",
    );
}

fn build_config(
    download_dir: &Path,
    db_path: &Path,
    workers: usize,
    rate_limit_kbps: Option<NonZeroU64>,
    file_conflict_strategy: FileConflictStrategy,
) -> Config {
    build_config_with_concurrency(
        download_dir,
        db_path,
        workers,
        1,
        rate_limit_kbps,
        file_conflict_strategy,
    )
}

fn build_config_with_concurrency(
    download_dir: &Path,
    db_path: &Path,
    workers: usize,
    concurrent_tasks: usize,
    rate_limit_kbps: Option<NonZeroU64>,
    file_conflict_strategy: FileConflictStrategy,
) -> Config {
    let mut config = ConfigBuilder::new()
        .download_dir(download_dir.to_path_buf())
        .segments_per_task(workers)
        .concurrent_tasks(concurrent_tasks)
        .rate_limit_kbps(rate_limit_kbps)
        .storage_backend(Backend::Sqlite(db_path.to_path_buf()))
        .file_conflict_strategy(file_conflict_strategy)
        .build()
        .unwrap();
    config.http.client.proxy.use_env_proxy = false;
    config
}

async fn wait_for_task_completion(
    task_id: u32,
    rx: &mut broadcast::Receiver<Event>,
) -> Result<(), String> {
    wait_for_task_set_completion(&[task_id], rx).await
}

async fn wait_for_task_set_completion(
    task_ids: &[u32],
    rx: &mut broadcast::Receiver<Event>,
) -> Result<(), String> {
    let mut pending = task_ids.to_vec();
    timeout(Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Ok(Event::Complete(id)) => {
                    pending.retain(|task_id| *task_id != id);
                    if pending.is_empty() {
                        return Ok(());
                    }
                }
                Ok(Event::Error(id, err)) if task_ids.contains(&id) => {
                    return Err(format!("task failed: {err}"));
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    return Err("event channel closed".into());
                }
            }
        }
    })
    .await
    .map_err(|_| "timed out waiting for task completion".to_string())?
}

struct TestDownloadServer {
    addr: std::net::SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl TestDownloadServer {
    async fn spawn(body: Arc<Vec<u8>>, response_chunk_size: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(stream) => stream,
                    Err(_) => break,
                };

                let body = Arc::clone(&body);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    loop {
                        let read = socket.read(&mut buffer).await.unwrap_or(0);
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                        if request.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }

                    let request_text = String::from_utf8_lossy(&request);
                    let first_line = request_text.lines().next().unwrap_or_default();
                    let range = extract_range(&request_text);

                    if first_line.starts_with("HEAD ") {
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                        return;
                    }

                    if let Some((start, end)) = range {
                        let end = end.min(body.len().saturating_sub(1));
                        let slice = &body[start..=end];
                        let header = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            slice.len(),
                            start,
                            end,
                            body.len()
                        );
                        let _ = socket.write_all(header.as_bytes()).await;
                        write_body_in_chunks(&mut socket, slice, response_chunk_size).await;
                        return;
                    }

                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    write_body_in_chunks(&mut socket, &body, response_chunk_size).await;
                });
            }
        });

        Self { addr, handle }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

impl Drop for TestDownloadServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn write_body_in_chunks(socket: &mut tokio::net::TcpStream, body: &[u8], chunk_size: usize) {
    for chunk in body.chunks(chunk_size.max(1)) {
        if socket.write_all(chunk).await.is_err() {
            return;
        }
    }
}

fn extract_range(request: &str) -> Option<(usize, usize)> {
    let header = request
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("range: bytes="))?;
    let range = header.split_once('=').map(|(_, value)| value.trim())?;
    let (start, end) = range.split_once('-')?;

    Some((start.parse().ok()?, end.parse().ok()?))
}
