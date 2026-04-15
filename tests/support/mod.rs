use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep};

#[derive(Debug, Clone)]
pub struct TestAsset {
    path: String,
    body: Arc<Vec<u8>>,
}

impl TestAsset {
    pub fn from_shared(path: impl Into<String>, body: Arc<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            body,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MultiFileServerConfig {
    pub response_chunk_size: usize,
    pub chunk_delay: Duration,
}

impl Default for MultiFileServerConfig {
    fn default() -> Self {
        Self {
            response_chunk_size: 4 * 1024,
            chunk_delay: Duration::from_millis(5),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub range: Option<String>,
}

pub struct MultiFileTestServer {
    addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    max_active_transfers: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

impl MultiFileTestServer {
    pub async fn spawn(assets: Vec<TestAsset>, config: MultiFileServerConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let asset_map = Arc::new(
            assets
                .into_iter()
                .map(|asset| (asset.path, asset.body))
                .collect::<HashMap<_, _>>(),
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let active_transfers = Arc::new(AtomicUsize::new(0));
        let max_active_transfers = Arc::new(AtomicUsize::new(0));

        let requests_for_task = Arc::clone(&requests);
        let active_for_task = Arc::clone(&active_transfers);
        let max_active_for_task = Arc::clone(&max_active_transfers);

        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(stream) => stream,
                    Err(_) => break,
                };

                let asset_map = Arc::clone(&asset_map);
                let requests = Arc::clone(&requests_for_task);
                let active_transfers = Arc::clone(&active_for_task);
                let max_active_transfers = Arc::clone(&max_active_for_task);

                tokio::spawn(async move {
                    handle_client(
                        &mut socket,
                        &asset_map,
                        &requests,
                        &active_transfers,
                        &max_active_transfers,
                        config,
                    )
                    .await;
                });
            }
        });

        Self {
            addr,
            requests,
            max_active_transfers,
            handle,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub async fn recorded_requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().await.clone()
    }

    pub async fn range_get_count(&self, path: &str) -> usize {
        self.recorded_requests()
            .await
            .into_iter()
            .filter(|request| {
                request.method == "GET" && request.path == path && request.range.is_some()
            })
            .count()
    }

    pub fn max_active_transfers(&self) -> usize {
        self.max_active_transfers.load(Ordering::SeqCst)
    }
}

impl Drop for MultiFileTestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn handle_client(
    socket: &mut tokio::net::TcpStream,
    assets: &HashMap<String, Arc<Vec<u8>>>,
    requests: &Arc<Mutex<Vec<RecordedRequest>>>,
    active_transfers: &Arc<AtomicUsize>,
    max_active_transfers: &Arc<AtomicUsize>,
    config: MultiFileServerConfig,
) {
    let request = read_http_request(socket).await;
    let Some((method, path)) = parse_request_line(&request) else {
        return;
    };
    let normalized_path = strip_query(&path);
    let range = extract_header(&request, "range");

    requests.lock().await.push(RecordedRequest {
        method: method.clone(),
        path: normalized_path.to_string(),
        range: range.clone(),
    });

    let Some(body) = assets.get(normalized_path) else {
        let _ = socket
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    };

    match method.as_str() {
        "HEAD" => {
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
        "GET" => {
            let _guard = ActiveTransferGuard::new(active_transfers, max_active_transfers);

            if let Some((start, end)) = extract_range_bounds(range.as_deref(), body.len()) {
                let slice = &body[start..=end];
                let response = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                    slice.len(),
                    start,
                    end,
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                write_body_in_chunks(socket, slice, config).await;
                return;
            }

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            write_body_in_chunks(socket, body, config).await;
        }
        _ => {
            let _ = socket
                .write_all(
                    b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
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

fn strip_query(path: &str) -> &str {
    path.split_once('?').map(|(path, _)| path).unwrap_or(path)
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

async fn write_body_in_chunks(
    socket: &mut tokio::net::TcpStream,
    body: &[u8],
    config: MultiFileServerConfig,
) {
    for chunk in body.chunks(config.response_chunk_size.max(1)) {
        if socket.write_all(chunk).await.is_err() {
            return;
        }
        if !config.chunk_delay.is_zero() {
            sleep(config.chunk_delay).await;
        }
    }
}

struct ActiveTransferGuard<'a> {
    active: &'a AtomicUsize,
}

impl<'a> ActiveTransferGuard<'a> {
    fn new(active: &'a AtomicUsize, max_active: &'a AtomicUsize) -> Self {
        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
        loop {
            let previous = max_active.load(Ordering::SeqCst);
            if previous >= current {
                break;
            }
            if max_active
                .compare_exchange(previous, current, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
        }

        Self { active }
    }
}

impl Drop for ActiveTransferGuard<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}
