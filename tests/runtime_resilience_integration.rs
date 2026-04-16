use paradown::download::{DownloadSpec, Manager};
use paradown::{Backend, Config};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{Duration, sleep};

#[tokio::test]
async fn retries_download_when_origin_drops_connection() {
    let payload = Arc::new(vec![0xAB; 64 * 1024]);
    let server = FlakyHttpServer::spawn(Arc::clone(&payload), FailureMode::FailFirstN(2)).await;
    let temp = tempdir().unwrap();

    let mut config = Config::default();
    config.download_dir = temp.path().join("downloads");
    config.storage_backend = Backend::Memory;
    config.segments_per_task = 1;
    config.concurrent_tasks = 1;
    config.retry.max_retries = 4;
    config.http.client.proxy.use_env_proxy = false;

    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let task_id = manager
        .add_download(DownloadSpec::parse(server.url()).unwrap())
        .await
        .unwrap();
    manager.start_task(task_id).await.unwrap();
    manager.wait_for_all_tasks().await.unwrap();

    let snapshot = manager.get_session_by_id(task_id).unwrap().snapshot().await;
    assert_eq!(snapshot.status, "Completed");
    assert!(snapshot.stats.retry_count >= 1);

    let file_name = snapshot.file_name.unwrap();
    let file_path = temp.path().join("downloads").join(file_name);
    let actual = tokio::fs::read(file_path).await.unwrap();
    assert_eq!(actual, payload.as_ref().as_slice());
}

#[tokio::test]
async fn retries_http_429_after_retry_after_delay() {
    let payload = Arc::new(vec![0xEF; 24 * 1024]);
    let server = FlakyHttpServer::spawn(
        Arc::clone(&payload),
        FailureMode::StatusFirstN {
            status: 429,
            retry_after_secs: Some(1),
            failures: 1,
        },
    )
    .await;
    let temp = tempdir().unwrap();

    let mut config = Config::default();
    config.download_dir = temp.path().join("downloads");
    config.storage_backend = Backend::Memory;
    config.segments_per_task = 1;
    config.concurrent_tasks = 1;
    config.retry.max_retries = 3;
    config.http.client.proxy.use_env_proxy = false;

    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let task_id = manager
        .add_download(DownloadSpec::parse(server.url()).unwrap())
        .await
        .unwrap();

    let started = std::time::Instant::now();
    manager.start_task(task_id).await.unwrap();
    manager.wait_for_all_tasks().await.unwrap();

    let snapshot = manager.get_session_by_id(task_id).unwrap().snapshot().await;
    assert_eq!(snapshot.status, "Completed");
    assert!(snapshot.stats.retry_count >= 1);
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "Retry-After should delay the follow-up request",
    );
    assert_eq!(server.download_attempts(), 2);
}

#[tokio::test]
async fn does_not_retry_non_retryable_http_statuses() {
    let payload = Arc::new(vec![0x11; 4 * 1024]);
    let server = FlakyHttpServer::spawn(
        payload,
        FailureMode::StatusFirstN {
            status: 404,
            retry_after_secs: None,
            failures: usize::MAX,
        },
    )
    .await;
    let temp = tempdir().unwrap();

    let mut config = Config::default();
    config.download_dir = temp.path().join("downloads");
    config.storage_backend = Backend::Memory;
    config.segments_per_task = 1;
    config.concurrent_tasks = 1;
    config.retry.max_retries = 3;
    config.http.client.proxy.use_env_proxy = false;

    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let task_id = manager
        .add_download(DownloadSpec::parse(server.url()).unwrap())
        .await
        .unwrap();
    manager.start_task(task_id).await.unwrap();
    manager.wait_for_all_tasks().await.unwrap();

    let snapshot = wait_for_snapshot_status(&manager, task_id, "Failed").await;
    assert!(snapshot.status.starts_with("Failed"));
    assert_eq!(snapshot.stats.retry_count, 0);
    assert_eq!(server.download_attempts(), 1);
}

#[tokio::test]
async fn writes_failure_diagnostic_after_retry_exhaustion() {
    let payload = Arc::new(vec![0xCD; 16 * 1024]);
    let server = FlakyHttpServer::spawn(payload, FailureMode::AlwaysDrop).await;
    let temp = tempdir().unwrap();

    let mut config = Config::default();
    config.download_dir = temp.path().join("downloads");
    config.storage_backend = Backend::Memory;
    config.segments_per_task = 1;
    config.concurrent_tasks = 1;
    config.retry.max_retries = 1;
    config.http.client.proxy.use_env_proxy = false;

    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let task_id = manager
        .add_download(DownloadSpec::parse(server.url()).unwrap())
        .await
        .unwrap();
    manager.start_task(task_id).await.unwrap();
    manager.wait_for_all_tasks().await.unwrap();

    let snapshot = manager.get_session_by_id(task_id).unwrap().snapshot().await;
    assert!(snapshot.status.starts_with("Failed"));

    let diagnostic_path = temp
        .path()
        .join("downloads")
        .join(".paradown")
        .join("diagnostics")
        .join(format!("task-{}.json", task_id));
    let diagnostic = wait_for_file(&diagnostic_path).await;
    assert!(diagnostic.contains("\"trace_id\""));
    assert!(diagnostic.contains("\"error\""));
}

#[derive(Clone, Copy)]
enum FailureMode {
    FailFirstN(usize),
    AlwaysDrop,
    StatusFirstN {
        status: u16,
        retry_after_secs: Option<u64>,
        failures: usize,
    },
}

struct FlakyHttpServer {
    addr: SocketAddr,
    download_attempts: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

impl FlakyHttpServer {
    async fn spawn(body: Arc<Vec<u8>>, failure_mode: FailureMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let download_attempts = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::clone(&download_attempts);

        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(stream) => stream,
                    Err(_) => break,
                };

                let body = Arc::clone(&body);
                let request_count = Arc::clone(&request_count);
                tokio::spawn(async move {
                    let request = read_http_request(&mut socket).await;
                    let Some((method, _path)) = parse_request_line(&request) else {
                        return;
                    };
                    let range = extract_header(&request, "range");
                    let is_probe_request = matches!(range.as_deref(), Some("bytes=0-0"));

                    if method == "HEAD" {
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                        return;
                    }

                    let (start, end) = extract_range_bounds(range.as_deref(), body.len())
                        .unwrap_or((0, body.len().saturating_sub(1)));
                    let slice = &body[start..=end];
                    let attempt = if is_probe_request {
                        0
                    } else {
                        request_count.fetch_add(1, Ordering::SeqCst) + 1
                    };
                    if !is_probe_request
                        && let FailureMode::StatusFirstN {
                            status,
                            retry_after_secs,
                            failures,
                        } = failure_mode
                        && attempt <= failures
                    {
                        let reason = http_reason_phrase(status);
                        let mut response = format!(
                            "HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n"
                        );
                        if let Some(retry_after_secs) = retry_after_secs {
                            response.push_str(&format!("Retry-After: {retry_after_secs}\r\n"));
                        }
                        response.push_str("\r\n");
                        let _ = socket.write_all(response.as_bytes()).await;
                        return;
                    }
                    let should_drop = match failure_mode {
                        FailureMode::FailFirstN(limit) if !is_probe_request => attempt <= limit,
                        FailureMode::AlwaysDrop => true,
                        FailureMode::StatusFirstN { .. } => false,
                        FailureMode::FailFirstN(_) => false,
                    };

                    let response = if range.is_some() {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            slice.len(),
                            start,
                            end,
                            body.len()
                        )
                    } else {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            slice.len()
                        )
                    };

                    if socket.write_all(response.as_bytes()).await.is_err() {
                        return;
                    }

                    if should_drop {
                        let partial = &slice[..slice.len().min(1024)];
                        let _ = socket.write_all(partial).await;
                        let _ = socket.shutdown().await;
                        return;
                    }

                    let _ = socket.write_all(slice).await;
                });
            }
        });

        Self {
            addr,
            download_attempts,
            handle,
        }
    }

    fn url(&self) -> String {
        format!("http://{}/flaky.bin", self.addr)
    }

    fn download_attempts(&self) -> usize {
        self.download_attempts.load(Ordering::SeqCst)
    }
}

impl Drop for FlakyHttpServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 2048];

    loop {
        let read = socket.read(&mut buffer).await.unwrap_or(0);
        if read == 0 {
            return request;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return request;
        }
    }
}

fn parse_request_line(request: &[u8]) -> Option<(String, String)> {
    let request_text = String::from_utf8_lossy(request);
    let mut parts = request_text.lines().next()?.split_whitespace();
    Some((parts.next()?.to_string(), parts.next()?.to_string()))
}

fn extract_header(request: &[u8], header_name: &str) -> Option<String> {
    let request_text = String::from_utf8_lossy(request);
    request_text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case(header_name) {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn extract_range_bounds(range: Option<&str>, body_len: usize) -> Option<(usize, usize)> {
    let range = range?;
    let range = range.strip_prefix("bytes=").unwrap_or(range);
    let (start, end) = range.split_once('-')?;

    let start = start.parse::<usize>().ok()?;
    let end = if end.is_empty() {
        body_len.checked_sub(1)?
    } else {
        end.parse::<usize>().ok()?
    };
    let capped_end = end.min(body_len.checked_sub(1)?);
    if start > capped_end {
        return None;
    }

    Some((start, capped_end))
}

fn http_reason_phrase(status: u16) -> &'static str {
    match status {
        404 => "Not Found",
        408 => "Request Timeout",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

async fn wait_for_file(path: &std::path::Path) -> String {
    for _ in 0..20 {
        if let Ok(contents) = tokio::fs::read_to_string(path).await {
            return contents;
        }
        sleep(Duration::from_millis(50)).await;
    }

    tokio::fs::read_to_string(path).await.unwrap()
}

async fn wait_for_snapshot_status(
    manager: &Manager,
    task_id: u32,
    expected_prefix: &str,
) -> paradown::download::SessionSnapshot {
    for _ in 0..50 {
        let snapshot = manager.get_session_by_id(task_id).unwrap().snapshot().await;
        if snapshot.status.starts_with(expected_prefix) {
            return snapshot;
        }
        sleep(Duration::from_millis(20)).await;
    }

    manager.get_session_by_id(task_id).unwrap().snapshot().await
}
