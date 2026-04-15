use paradown::download::{DownloadSpec, Event, Manager};
use paradown::{Backend, Config, ConfigBuilder, FileConflictStrategy, Store};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::time::{Duration, timeout};

#[tokio::test]
async fn follows_redirect_and_uses_content_disposition_filename() {
    let body = Arc::new(b"redirected-payload".to_vec());
    let server = MetadataTestServer::spawn(
        MetadataMode::RedirectToDisposition {
            body: Arc::clone(&body),
            file_name: "release-header.bin",
            final_path: "/files/final.bin?token=abc",
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
        FileConflictStrategy::Overwrite,
    );
    let manager = Manager::new(config.clone()).unwrap();
    manager.init().await.unwrap();

    let mut rx = manager.subscribe_events();
    let task_id = manager
        .add_download(DownloadSpec::parse(server.url("/start")).unwrap())
        .await
        .unwrap();
    manager.start_task(task_id).await.unwrap();
    wait_for_task_completion(task_id, &mut rx).await.unwrap();

    let downloaded = tokio::fs::read(download_dir.join("release-header.bin"))
        .await
        .unwrap();
    assert_eq!(downloaded, *body);

    let persistence = Store::new(Arc::new(config)).await.unwrap();
    let task = persistence.load_task(task_id).await.unwrap().unwrap();
    assert!(task.resolved_url.contains("/files/final.bin?token=abc"));
    assert_eq!(task.file_name, "release-header.bin");
}

#[tokio::test]
async fn surfaces_clear_error_when_content_length_is_missing() {
    let server = MetadataTestServer::spawn(MetadataMode::MissingContentLength {
        body: Arc::new(b"stream-without-length".to_vec()),
    })
    .await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();

    let config = build_config(
        &download_dir,
        &db_path,
        FileConflictStrategy::Overwrite,
    );
    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let task_id = manager
        .add_download(DownloadSpec::parse(server.url("/stream")).unwrap())
        .await
        .unwrap();
    let err = manager.start_task(task_id).await.unwrap_err();
    assert!(err.to_string().contains("Content-Length"));
}

#[tokio::test]
async fn redownloads_existing_file_when_no_validator_can_prove_it_is_valid() {
    let body = Arc::new(b"correct-payload".to_vec());
    let server = MetadataTestServer::spawn(MetadataMode::PlainHttp {
        body: Arc::clone(&body),
        path: "/payload.bin",
        content_disposition: None,
    })
    .await;

    let sandbox = TempDir::new().unwrap();
    let download_dir = sandbox.path().join("downloads");
    let db_path = sandbox.path().join("state.db");
    tokio::fs::create_dir_all(&download_dir).await.unwrap();
    tokio::fs::write(download_dir.join("payload.bin"), b"wrong-file-size")
        .await
        .unwrap();

    let config = build_config(
        &download_dir,
        &db_path,
        FileConflictStrategy::SkipIfValid,
    );
    let manager = Manager::new(config).unwrap();
    manager.init().await.unwrap();

    let mut rx = manager.subscribe_events();
    let task_id = manager
        .add_download(DownloadSpec::parse(server.url("/payload.bin")).unwrap())
        .await
        .unwrap();
    manager.start_task(task_id).await.unwrap();
    wait_for_task_completion(task_id, &mut rx).await.unwrap();

    let downloaded = tokio::fs::read(download_dir.join("payload.bin"))
        .await
        .unwrap();
    assert_eq!(downloaded, *body);
}

fn build_config(download_dir: &Path, db_path: &Path, strategy: FileConflictStrategy) -> Config {
    ConfigBuilder::new()
        .download_dir(download_dir.to_path_buf())
        .segments_per_task(2)
        .concurrent_tasks(1)
        .storage_backend(Backend::Sqlite(db_path.to_path_buf()))
        .file_conflict_strategy(strategy)
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

enum MetadataMode {
    RedirectToDisposition {
        body: Arc<Vec<u8>>,
        file_name: &'static str,
        final_path: &'static str,
    },
    MissingContentLength {
        body: Arc<Vec<u8>>,
    },
    PlainHttp {
        body: Arc<Vec<u8>>,
        path: &'static str,
        content_disposition: Option<&'static str>,
    },
}

struct MetadataTestServer {
    addr: std::net::SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl MetadataTestServer {
    async fn spawn(mode: MetadataMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(stream) => stream,
                    Err(_) => break,
                };

                match &mode {
                    MetadataMode::RedirectToDisposition {
                        body,
                        file_name,
                        final_path,
                    } => {
                        let body = Arc::clone(body);
                        let final_path = (*final_path).to_string();
                        let file_name = (*file_name).to_string();
                        tokio::spawn(async move {
                            handle_redirect_request(
                                &mut socket,
                                &body,
                                &final_path,
                                &file_name,
                            )
                            .await;
                        });
                    }
                    MetadataMode::MissingContentLength { body } => {
                        let body = Arc::clone(body);
                        tokio::spawn(async move {
                            handle_missing_length_request(&mut socket, &body).await;
                        });
                    }
                    MetadataMode::PlainHttp {
                        body,
                        path,
                        content_disposition,
                    } => {
                        let body = Arc::clone(body);
                        let path = (*path).to_string();
                        let content_disposition = content_disposition.map(str::to_string);
                        tokio::spawn(async move {
                            handle_plain_request(
                                &mut socket,
                                &body,
                                &path,
                                content_disposition.as_deref(),
                            )
                            .await;
                        });
                    }
                }
            }
        });

        Self { addr, handle }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

impl Drop for MetadataTestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn handle_redirect_request(
    socket: &mut tokio::net::TcpStream,
    body: &[u8],
    final_path: &str,
    file_name: &str,
) {
    let request = read_http_request(socket).await;
    let (method, path) = parse_request_line(&request);

    if path == "/start" {
        let response = format!(
            "HTTP/1.1 302 Found\r\nLocation: {}\r\nConnection: close\r\n\r\n",
            final_path
        );
        let _ = socket.write_all(response.as_bytes()).await;
        return;
    }

    if path == final_path {
        write_http_payload(
            socket,
            &request,
            method,
            body,
            Some(file_name),
            true,
            true,
            Some("\"etag-redirect\""),
        )
        .await;
    }
}

async fn handle_missing_length_request(socket: &mut tokio::net::TcpStream, body: &[u8]) {
    let request = read_http_request(socket).await;
    let (method, _) = parse_request_line(&request);

    if method == "HEAD" {
        let response = "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
        let _ = socket.write_all(response.as_bytes()).await;
        return;
    }

    let response = "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.write_all(body).await;
}

async fn handle_plain_request(
    socket: &mut tokio::net::TcpStream,
    body: &[u8],
    expected_path: &str,
    content_disposition: Option<&str>,
) {
    let request = read_http_request(socket).await;
    let (method, path) = parse_request_line(&request);
    if path != expected_path {
        let _ = socket
            .write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
            .await;
        return;
    }

    write_http_payload(
        socket,
        &request,
        method,
        body,
        content_disposition,
        true,
        true,
        None,
    )
    .await;
}

async fn write_http_payload(
    socket: &mut tokio::net::TcpStream,
    request: &str,
    method: &str,
    body: &[u8],
    content_disposition: Option<&str>,
    include_length: bool,
    accept_ranges: bool,
    entity_tag: Option<&str>,
) {
    let mut headers = String::new();
    if accept_ranges {
        headers.push_str("Accept-Ranges: bytes\r\n");
    }
    if let Some(content_disposition) = content_disposition {
        headers.push_str(&format!(
            "Content-Disposition: attachment; filename=\"{}\"\r\n",
            content_disposition
        ));
    }
    if let Some(entity_tag) = entity_tag {
        headers.push_str(&format!("ETag: {}\r\n", entity_tag));
    }

    let range = extract_header_from_request(request, "range")
        .and_then(|value| parse_range_header(&value))
        .filter(|_| method != "HEAD");
    let (status_line, response_body, response_headers) = if let Some((start, end)) = range {
        let end = end.min(body.len().saturating_sub(1));
        let slice = &body[start..=end];
        let mut response_headers = headers.clone();
        response_headers.push_str(&format!("Content-Length: {}\r\n", slice.len()));
        response_headers.push_str(&format!(
            "Content-Range: bytes {}-{}/{}\r\n",
            start,
            end,
            body.len()
        ));
        ("HTTP/1.1 206 Partial Content", slice.to_vec(), response_headers)
    } else {
        if include_length {
            headers.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        ("HTTP/1.1 200 OK", body.to_vec(), headers)
    };

    let response = format!(
        "{}\r\n{}Connection: close\r\n\r\n",
        status_line, response_headers
    );
    let _ = socket.write_all(response.as_bytes()).await;
    if method != "HEAD" {
        let _ = socket.write_all(&response_body).await;
    }
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0u8; 2048];
    loop {
        let read = socket.read(&mut buffer).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    String::from_utf8_lossy(&request).to_string()
}

fn parse_request_line(request: &str) -> (&str, &str) {
    let first_line = request.lines().next().unwrap_or_default();
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    (method, path)
}

fn extract_header_from_request(request: &str, name: &str) -> Option<String> {
    let expected = format!("{}:", name.to_ascii_lowercase());
    request.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with(&expected) {
            line.split_once(':').map(|(_, value)| value.trim().to_string())
        } else {
            None
        }
    })
}

fn parse_range_header(value: &str) -> Option<(usize, usize)> {
    let value = value.trim().strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?))
}
