use paradown::download::{Event, Manager};
use paradown::repository::models::{DBDownloadTask, DBDownloadWorker};
use paradown::{Backend, Config, ConfigBuilder, FileConflictStrategy, Store};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast};
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn resume_requests_include_if_range_validator() {
    let body = Arc::new(b"abcdefghij".to_vec());
    let server = ResumeTestServer::spawn(Arc::clone(&body), "\"etag-v1\"").await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let file_path = download_dir.join("file.bin");
    tokio::fs::write(&file_path, &body[..5]).await.unwrap();

    seed_paused_resume_task(
        &download_dir,
        &db_path,
        &file_path,
        &server.url("/file.bin"),
        "\"etag-v1\"",
        5,
        body.len() as u64,
    )
    .await;

    let manager = Manager::new(build_config(&download_dir, &db_path)).unwrap();
    manager.init().await.unwrap();

    let mut rx = manager.subscribe_events();
    manager.resume_task(1).await.unwrap();
    wait_for_task_completion(1, &mut rx).await.unwrap();

    let downloaded = tokio::fs::read(&file_path).await.unwrap();
    assert_eq!(downloaded, *body);

    let requests = server.recorded_requests().await;
    assert!(
        requests.iter().any(|request| {
            request.path == "/file.bin"
                && request.method == "GET"
                && request.range.as_deref() == Some("bytes=5-9")
                && request.if_range.as_deref() == Some("\"etag-v1\"")
        }),
        "requests: {requests:?}"
    );
}

#[tokio::test]
async fn resume_restarts_from_scratch_when_remote_validator_changes() {
    let body = Arc::new(b"updated-body".to_vec());
    let server = ResumeTestServer::spawn(Arc::clone(&body), "\"etag-v2\"").await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let file_path = download_dir.join("file.bin");
    tokio::fs::write(&file_path, b"old-b").await.unwrap();

    seed_paused_resume_task(
        &download_dir,
        &db_path,
        &file_path,
        &server.url("/file.bin"),
        "\"etag-v1\"",
        5,
        body.len() as u64,
    )
    .await;

    let manager = Manager::new(build_config(&download_dir, &db_path)).unwrap();
    manager.init().await.unwrap();

    let mut rx = manager.subscribe_events();
    manager.resume_task(1).await.unwrap();
    wait_for_task_completion(1, &mut rx).await.unwrap();

    let downloaded = tokio::fs::read(&file_path).await.unwrap();
    assert_eq!(downloaded, *body);

    let requests = server.recorded_requests().await;
    assert!(
        requests.iter().any(|request| {
            request.path == "/file.bin"
                && request.method == "GET"
                && request.range.as_deref() == Some("bytes=0-11")
                && request.if_range.is_none()
        }),
        "requests: {requests:?}"
    );
}

async fn seed_paused_resume_task(
    download_dir: &Path,
    db_path: &Path,
    file_path: &Path,
    locator: &str,
    entity_tag: &str,
    downloaded: u64,
    total_size: u64,
) {
    let config = build_config(download_dir, db_path);
    let persistence = Store::new(Arc::new(config)).await.unwrap();
    let repository = persistence.repository();

    repository
        .save_task(&DBDownloadTask {
            id: 1,
            url: locator.to_string(),
            resolved_url: locator.to_string(),
            entity_tag: entity_tag.into(),
            last_modified: "".into(),
            file_name: "file.bin".into(),
            file_path: file_path.to_string_lossy().to_string(),
            status: "Paused".into(),
            downloaded_size: downloaded,
            total_size: Some(total_size),
            created_at: None,
            updated_at: None,
        })
        .await
        .unwrap();
    repository
        .save_worker(&DBDownloadWorker {
            id: 1,
            task_id: 1,
            index: 0,
            start: 0,
            end: total_size.saturating_sub(1),
            downloaded,
            status: "Paused".into(),
            updated_at: None,
        })
        .await
        .unwrap();
}

fn build_config(download_dir: &Path, db_path: &Path) -> Config {
    ConfigBuilder::new()
        .download_dir(download_dir.to_path_buf())
        .segments_per_task(1)
        .concurrent_tasks(1)
        .storage_backend(Backend::Sqlite(db_path.to_path_buf()))
        .file_conflict_strategy(FileConflictStrategy::Resume)
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedRequest {
    method: String,
    path: String,
    range: Option<String>,
    if_range: Option<String>,
}

struct ResumeTestServer {
    addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl ResumeTestServer {
    async fn spawn(body: Arc<Vec<u8>>, entity_tag: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_task = Arc::clone(&requests);

        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(stream) => stream,
                    Err(_) => break,
                };

                let body = Arc::clone(&body);
                let requests = Arc::clone(&requests_for_task);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 2048];
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
                    let mut first_line_parts = first_line.split_whitespace();
                    let method = first_line_parts.next().unwrap_or_default().to_string();
                    let path = first_line_parts.next().unwrap_or_default().to_string();
                    let range = extract_header(&request_text, "range");
                    let if_range = extract_header(&request_text, "if-range");

                    requests.lock().await.push(RecordedRequest {
                        method: method.clone(),
                        path: path.clone(),
                        range: range.clone(),
                        if_range: if_range.clone(),
                    });

                    if method == "HEAD" {
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: {}\r\nConnection: close\r\n\r\n",
                            body.len(),
                            entity_tag
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                        return;
                    }

                    if let Some(range_header) = range {
                        let (start, end) = parse_range(&range_header);
                        let if_range_matches = if_range
                            .as_deref()
                            .map(|value| value == entity_tag)
                            .unwrap_or(true);
                        if if_range_matches {
                            let end = end.min(body.len().saturating_sub(1));
                            let slice = &body[start..=end];
                            let response = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nETag: {}\r\nConnection: close\r\n\r\n",
                                slice.len(),
                                start,
                                end,
                                body.len(),
                                entity_tag
                            );
                            let _ = socket.write_all(response.as_bytes()).await;
                            let _ = socket.write_all(slice).await;
                            return;
                        }

                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: {}\r\nConnection: close\r\n\r\n",
                            body.len(),
                            entity_tag
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                        let _ = socket.write_all(&body).await;
                        return;
                    }

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: {}\r\nConnection: close\r\n\r\n",
                        body.len(),
                        entity_tag
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });

        Self {
            addr,
            requests,
            handle,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    async fn recorded_requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().await.clone()
    }
}

impl Drop for ResumeTestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn extract_header(request: &str, name: &str) -> Option<String> {
    let expected = format!("{}:", name.to_ascii_lowercase());
    request.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with(&expected) {
            line.split_once(':')
                .map(|(_, value)| value.trim().to_string())
        } else {
            None
        }
    })
}

fn parse_range(value: &str) -> (usize, usize) {
    let value = value.trim().strip_prefix("bytes=").unwrap();
    let (start, end) = value.split_once('-').unwrap();
    (start.parse().unwrap(), end.parse().unwrap())
}
