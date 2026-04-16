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

    let task = manager.get_task_by_id(task_id).unwrap();
    let snapshot = task.snapshot().await;
    assert_eq!(snapshot.status, "Completed");
    assert!(snapshot.stats.retry_count >= 1);

    let file_name = snapshot.file_name.unwrap();
    let file_path = temp.path().join("downloads").join(file_name);
    let actual = tokio::fs::read(file_path).await.unwrap();
    assert_eq!(actual, payload.as_ref().as_slice());
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

    let task = manager.get_task_by_id(task_id).unwrap();
    let snapshot = task.snapshot().await;
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
}

struct FlakyHttpServer {
    addr: SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl FlakyHttpServer {
    async fn spawn(body: Arc<Vec<u8>>, failure_mode: FailureMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let request_count = Arc::new(AtomicUsize::new(0));

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
                    let attempt = request_count.fetch_add(1, Ordering::SeqCst) + 1;
                    let should_drop = match failure_mode {
                        FailureMode::FailFirstN(limit) => attempt <= limit,
                        FailureMode::AlwaysDrop => true,
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

        Self { addr, handle }
    }

    fn url(&self) -> String {
        format!("http://{}/flaky.bin", self.addr)
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

async fn wait_for_file(path: &std::path::Path) -> String {
    for _ in 0..20 {
        if let Ok(contents) = tokio::fs::read_to_string(path).await {
            return contents;
        }
        sleep(Duration::from_millis(50)).await;
    }

    tokio::fs::read_to_string(path).await.unwrap()
}
