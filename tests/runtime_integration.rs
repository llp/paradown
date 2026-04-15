use paradown::FileConflictStrategy;
use paradown::download::{DownloadSpec, Event, Manager, Status};
use paradown::repository::models::{DBDownloadTask, DBDownloadWorker};
use paradown::{Backend, Config, ConfigBuilder, Store};
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
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

    let task = manager
        .get_task_by_id(7)
        .expect("restored task should exist");
    assert!(matches!(*task.status.lock().await, Status::Paused));
    assert_eq!(task.downloaded_size.load(Ordering::Relaxed), 60);

    let workers = task.workers.read().await;
    assert_eq!(workers.len(), 2);
    assert!(matches!(
        workers[1].status.lock().await.clone(),
        Status::Paused
    ));
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

    let task = manager.get_task_by_id(task_id).unwrap();
    assert!(matches!(*task.status.lock().await, Status::Completed));
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

fn build_config(
    download_dir: &Path,
    db_path: &Path,
    workers: usize,
    rate_limit_kbps: Option<NonZeroU64>,
    file_conflict_strategy: FileConflictStrategy,
) -> Config {
    ConfigBuilder::new()
        .download_dir(download_dir.to_path_buf())
        .segments_per_task(workers)
        .concurrent_tasks(1)
        .rate_limit_kbps(rate_limit_kbps)
        .storage_backend(Backend::Sqlite(db_path.to_path_buf()))
        .file_conflict_strategy(file_conflict_strategy)
        .build()
        .unwrap()
}

async fn wait_for_task_completion(
    task_id: u32,
    rx: &mut broadcast::Receiver<Event>,
) -> Result<(), String> {
    timeout(Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Ok(Event::Complete(id)) if id == task_id => return Ok(()),
                Ok(Event::Error(id, err)) if id == task_id => {
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
