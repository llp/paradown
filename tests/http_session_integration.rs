use paradown::download::{DownloadSpec, Event, Manager, SessionRequest};
use paradown::{
    Backend, Config, ConfigBuilder, FileConflictStrategy, SourceCapabilities, SourceDescriptor,
    SourceKind, SourceSet,
};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn persists_cookie_jar_across_manager_restarts() {
    let server = CookieSessionServer::spawn().await;
    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let cookie_jar = sandbox.path().join("cookies.json");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let mut config = build_config(&download_dir);
    config.http.client.cookie_store = true;
    config.http.client.cookie_jar_path = Some(cookie_jar.clone());

    let manager = Manager::new(config.clone()).unwrap();
    manager.init().await.unwrap();
    let mut rx = manager.subscribe_events();
    let login_task = manager
        .add_download(DownloadSpec::parse(server.url("/login.bin")).unwrap())
        .await
        .unwrap();
    manager.start_task(login_task).await.unwrap();
    wait_for_task_completion(login_task, &mut rx).await.unwrap();
    manager.wait_for_all_tasks().await.unwrap();
    drop(manager);

    let cookie_bytes = tokio::fs::read(&cookie_jar).await.unwrap();
    assert!(
        !cookie_bytes.is_empty(),
        "cookie jar should be persisted after terminal events",
    );

    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();
    let mut rx = manager.subscribe_events();
    let protected_task = manager
        .add_download(DownloadSpec::parse(server.url("/protected.bin")).unwrap())
        .await
        .unwrap();
    manager.start_task(protected_task).await.unwrap();
    wait_for_task_completion(protected_task, &mut rx)
        .await
        .expect("persisted cookie jar should authorize the protected download");

    let downloaded = tokio::fs::read(download_dir.join("protected.bin"))
        .await
        .unwrap();
    assert_eq!(downloaded, b"protected-payload");
    assert!(server.protected_hits() >= 1);
}

#[tokio::test]
async fn retries_failed_primary_source_on_mirror_origin() {
    let body = Arc::new(
        (0..(48 * 1024))
            .map(|idx| ((idx * 5) % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let primary = HttpAssetServer::spawn("/artifact.bin", Arc::clone(&body), true).await;
    let mirror = HttpAssetServer::spawn("/artifact.bin", Arc::clone(&body), false).await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let mut config = build_config(&download_dir);
    config.segments_per_task = 1;
    config.retry.max_retries = 3;

    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let primary_spec = DownloadSpec::parse(primary.url("/artifact.bin")).unwrap();
    let mirror_spec = DownloadSpec::parse(mirror.url("/artifact.bin")).unwrap();
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

    let downloaded = tokio::fs::read(download_dir.join("artifact.bin"))
        .await
        .unwrap();
    assert_eq!(downloaded, *body);
    assert_eq!(primary.data_requests(), 1);
    assert!(
        mirror.data_requests() >= 1,
        "mirror origin should serve the retried lane",
    );

    let snapshot = manager.get_session_by_id(task_id).unwrap().snapshot().await;
    assert_eq!(snapshot.status, "Completed");
    assert_eq!(snapshot.source_count, 2);
    assert!(snapshot.stats.retry_count >= 1);
}

fn build_config(download_dir: &std::path::Path) -> Config {
    let mut config = ConfigBuilder::new()
        .download_dir(download_dir.to_path_buf())
        .segments_per_task(1)
        .concurrent_tasks(1)
        .storage_backend(Backend::Memory)
        .file_conflict_strategy(FileConflictStrategy::Overwrite)
        .build()
        .unwrap();
    config.http.client.proxy.use_env_proxy = false;
    config
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

struct CookieSessionServer {
    addr: SocketAddr,
    protected_hits: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

impl CookieSessionServer {
    async fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let protected_hits = Arc::new(AtomicUsize::new(0));
        let protected_hits_handle = Arc::clone(&protected_hits);

        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(stream) => stream,
                    Err(_) => break,
                };
                let protected_hits = Arc::clone(&protected_hits_handle);

                tokio::spawn(async move {
                    let request = read_http_request(&mut socket).await;
                    let Some((method, path)) = parse_request_line(&request) else {
                        return;
                    };
                    let cookie = extract_header(&request, "cookie").unwrap_or_default();

                    let response = match (method, path.as_str()) {
                        ("HEAD", "/login.bin") => head_response_with_cookie(
                            b"login-seeded-cookie".len(),
                            Some("session=abc123; Path=/; Max-Age=3600"),
                        ),
                        ("GET", "/login.bin") => http_ok_response(
                            b"login-seeded-cookie",
                            Some("session=abc123; Path=/; Max-Age=3600"),
                        ),
                        ("HEAD", "/protected.bin") | ("GET", "/protected.bin")
                            if cookie.contains("session=abc123") =>
                        {
                            protected_hits.fetch_add(1, Ordering::SeqCst);
                            if method == "HEAD" {
                                head_response_with_cookie(b"protected-payload".len(), None)
                            } else {
                                http_ok_response(b"protected-payload", None)
                            }
                        }
                        _ => http_status_response(401, "Unauthorized"),
                    };

                    let _ = socket.write_all(&response).await;
                });
            }
        });

        Self {
            addr,
            protected_hits,
            handle,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn protected_hits(&self) -> usize {
        self.protected_hits.load(Ordering::SeqCst)
    }
}

impl Drop for CookieSessionServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

struct HttpAssetServer {
    addr: SocketAddr,
    data_requests: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

impl HttpAssetServer {
    async fn spawn(path: &'static str, body: Arc<Vec<u8>>, fail_first_data_request: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let data_requests = Arc::new(AtomicUsize::new(0));
        let data_requests_handle = Arc::clone(&data_requests);

        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(stream) => stream,
                    Err(_) => break,
                };
                let body = Arc::clone(&body);
                let data_requests = Arc::clone(&data_requests_handle);

                tokio::spawn(async move {
                    let request = read_http_request(&mut socket).await;
                    let Some((method, request_path)) = parse_request_line(&request) else {
                        return;
                    };
                    if request_path != path {
                        let _ = socket
                            .write_all(&http_status_response(404, "Not Found"))
                            .await;
                        return;
                    }

                    let range = extract_header(&request, "range");
                    let is_probe = matches!(range.as_deref(), Some("bytes=0-0"));
                    let request_index = if method == "GET" && !is_probe {
                        data_requests.fetch_add(1, Ordering::SeqCst) + 1
                    } else {
                        0
                    };

                    let response = if method == "HEAD" {
                        head_response(body.len())
                    } else if fail_first_data_request && request_index == 1 {
                        http_status_response(503, "Service Unavailable")
                    } else {
                        let (start, end) = extract_range_bounds(range.as_deref(), body.len())
                            .unwrap_or((0, body.len().saturating_sub(1)));
                        http_partial_response(&body, Some((start, end)))
                    };

                    let _ = socket.write_all(&response).await;
                });
            }
        });

        Self {
            addr,
            data_requests,
            handle,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn data_requests(&self) -> usize {
        self.data_requests.load(Ordering::SeqCst)
    }
}

impl Drop for HttpAssetServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = socket.read(&mut buf).await.unwrap();
        if n == 0 {
            break;
        }
        request.extend_from_slice(&buf[..n]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&request).into_owned()
}

fn parse_request_line(request: &str) -> Option<(&str, String)> {
    let mut parts = request.lines().next()?.split_whitespace();
    let method = parts.next()?;
    let path = parts.next()?.split('?').next()?.to_string();
    Some((method, path))
}

fn extract_header(request: &str, header_name: &str) -> Option<String> {
    request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case(header_name) {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn extract_range_bounds(range: Option<&str>, total_len: usize) -> Option<(usize, usize)> {
    let header = range?.strip_prefix("bytes=")?;
    let (start, end) = header.split_once('-')?;
    let start = start.parse::<usize>().ok()?;
    let requested_end = end.parse::<usize>().ok()?;
    let capped_end = requested_end.min(total_len.saturating_sub(1));
    if start > capped_end {
        return None;
    }
    Some((start, capped_end))
}

fn http_ok_response(body: &[u8], set_cookie: Option<&str>) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(set_cookie) = set_cookie {
        response.push_str(&format!("Set-Cookie: {set_cookie}\r\n"));
    }
    response.push_str("\r\n");
    let mut response = response.into_bytes();
    response.extend_from_slice(body);
    response
}

fn http_status_response(status: u16, reason: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

fn http_partial_response(body: &[u8], range: Option<(usize, usize)>) -> Vec<u8> {
    let (status_line, slice, range_header) = match range {
        Some((start, end)) => (
            "HTTP/1.1 206 Partial Content",
            &body[start..=end],
            Some(format!(
                "Content-Range: bytes {start}-{end}/{}\r\n",
                body.len()
            )),
        ),
        None => ("HTTP/1.1 200 OK", body, None),
    };

    let mut response = format!(
        "{status_line}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n",
        slice.len()
    );
    if let Some(range_header) = range_header {
        response.push_str(&range_header);
    }
    response.push_str("\r\n");

    let mut response = response.into_bytes();
    response.extend_from_slice(slice);
    response
}

fn head_response(total_len: usize) -> Vec<u8> {
    head_response_with_cookie(total_len, None)
}

fn head_response_with_cookie(total_len: usize, set_cookie: Option<&str>) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {total_len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n"
    );
    if let Some(set_cookie) = set_cookie {
        response.push_str(&format!("Set-Cookie: {set_cookie}\r\n"));
    }
    response.push_str("\r\n");
    response.into_bytes()
}
