use crate::error::Error;
use reqwest::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use reqwest::{Client, Response, StatusCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DownloadProtocolProbe {
    pub total_size: u64,
    pub supports_range_requests: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentRange {
    pub start: u64,
    pub end: u64,
    pub total_size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeadProbe {
    total_size: Option<u64>,
    accepts_ranges: bool,
}

pub(crate) async fn probe_download_target(
    client: &Client,
    url: &str,
) -> Result<DownloadProtocolProbe, Error> {
    let head_probe = probe_with_head(client, url).await?;

    if let Some(head) = head_probe {
        if head.total_size == Some(0) {
            return Ok(DownloadProtocolProbe {
                total_size: 0,
                supports_range_requests: head.accepts_ranges,
            });
        }
    }

    let response = client.get(url).header(RANGE, "bytes=0-0").send().await?;
    probe_with_range_response(response, head_probe)
}

pub(crate) fn parse_content_range(value: &str) -> Option<ContentRange> {
    let value = value.trim();
    let value = value.strip_prefix("bytes ")?;
    let (range_part, total_part) = value.split_once('/')?;
    let (start_part, end_part) = range_part.split_once('-')?;

    let start = start_part.parse().ok()?;
    let end = end_part.parse().ok()?;
    let total_size = if total_part == "*" {
        None
    } else {
        Some(total_part.parse().ok()?)
    };

    Some(ContentRange {
        start,
        end,
        total_size,
    })
}

async fn probe_with_head(client: &Client, url: &str) -> Result<Option<HeadProbe>, Error> {
    let response = match client.head(url).send().await {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };

    if !response.status().is_success() {
        return Ok(None);
    }

    Ok(Some(HeadProbe {
        total_size: parse_content_length(response.headers().get(CONTENT_LENGTH))?,
        accepts_ranges: header_accepts_ranges(response.headers().get(ACCEPT_RANGES)),
    }))
}

fn probe_with_range_response(
    response: Response,
    head_probe: Option<HeadProbe>,
) -> Result<DownloadProtocolProbe, Error> {
    match response.status() {
        StatusCode::PARTIAL_CONTENT => {
            let content_range = response
                .headers()
                .get(CONTENT_RANGE)
                .ok_or_else(|| Error::Other("Missing Content-Range".into()))?
                .to_str()?;
            let content_range = parse_content_range(content_range)
                .ok_or_else(|| Error::Other("Invalid Content-Range".into()))?;
            let total_size = content_range
                .total_size
                .ok_or_else(|| Error::Other("Content-Range missing total size".into()))?;

            Ok(DownloadProtocolProbe {
                total_size,
                supports_range_requests: true,
            })
        }
        StatusCode::OK => {
            let total_size = parse_content_length(response.headers().get(CONTENT_LENGTH))?
                .or_else(|| head_probe.and_then(|probe| probe.total_size))
                .ok_or_else(|| Error::Other("No Content-Length".into()))?;

            Ok(DownloadProtocolProbe {
                total_size,
                supports_range_requests: false,
            })
        }
        StatusCode::RANGE_NOT_SATISFIABLE => {
            let total_size = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_unsatisfied_content_range)
                .or_else(|| head_probe.and_then(|probe| probe.total_size))
                .unwrap_or(0);

            Ok(DownloadProtocolProbe {
                total_size,
                supports_range_requests: total_size > 0,
            })
        }
        status => Err(Error::Other(format!(
            "Failed to probe download target: {}",
            status
        ))),
    }
}

fn parse_unsatisfied_content_range(value: &str) -> Option<u64> {
    let value = value.trim();
    let value = value.strip_prefix("bytes */")?;
    value.parse().ok()
}

fn header_accepts_ranges(value: Option<&reqwest::header::HeaderValue>) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("bytes"))
        .unwrap_or(false)
}

fn parse_content_length(
    value: Option<&reqwest::header::HeaderValue>,
) -> Result<Option<u64>, Error> {
    value
        .map(|value| value.to_str()?.parse::<u64>().map_err(Error::from))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::{parse_content_range, probe_download_target};
    use reqwest::Client;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn parses_standard_content_range_header() {
        let range = parse_content_range("bytes 0-99/200").unwrap();
        assert_eq!(range.start, 0);
        assert_eq!(range.end, 99);
        assert_eq!(range.total_size, Some(200));
    }

    #[tokio::test]
    async fn detects_range_support_from_partial_response() {
        let server = TestHttpServer::spawn(TestServerMode::RangeSupported).await;
        let client = Client::new();

        let probe = probe_download_target(&client, &server.url()).await.unwrap();
        assert_eq!(probe.total_size, 5);
        assert!(probe.supports_range_requests);
    }

    #[tokio::test]
    async fn falls_back_to_single_stream_when_range_is_ignored() {
        let server = TestHttpServer::spawn(TestServerMode::RangeIgnored).await;
        let client = Client::new();

        let probe = probe_download_target(&client, &server.url()).await.unwrap();
        assert_eq!(probe.total_size, 5);
        assert!(!probe.supports_range_requests);
    }

    #[tokio::test]
    async fn handles_head_rejection_with_range_probe() {
        let server = TestHttpServer::spawn(TestServerMode::HeadRejected).await;
        let client = Client::new();

        let probe = probe_download_target(&client, &server.url()).await.unwrap();
        assert_eq!(probe.total_size, 5);
        assert!(probe.supports_range_requests);
    }

    struct TestHttpServer {
        addr: SocketAddr,
        handle: tokio::task::JoinHandle<()>,
    }

    impl TestHttpServer {
        async fn spawn(mode: TestServerMode) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let body = Arc::new(b"hello".to_vec());

            let handle = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = match listener.accept().await {
                        Ok(stream) => stream,
                        Err(_) => break,
                    };

                    let body = Arc::clone(&body);
                    let mode = mode;
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
                        let has_range = request_text
                            .lines()
                            .any(|line| line.to_ascii_lowercase().starts_with("range: bytes=0-0"));

                        let response = if first_line.starts_with("HEAD ") {
                            build_head_response(mode, body.len())
                        } else if first_line.starts_with("GET ") && has_range {
                            build_range_response(mode, &body)
                        } else {
                            build_full_response(&body)
                        };

                        let _ = socket.write_all(response.as_bytes()).await;
                    });
                }
            });

            Self { addr, handle }
        }

        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    impl Drop for TestHttpServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    #[derive(Clone, Copy)]
    enum TestServerMode {
        RangeSupported,
        RangeIgnored,
        HeadRejected,
    }

    fn build_head_response(mode: TestServerMode, body_len: usize) -> String {
        match mode {
            TestServerMode::RangeSupported => format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {body_len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
            ),
            TestServerMode::RangeIgnored => format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {body_len}\r\nAccept-Ranges: none\r\nConnection: close\r\n\r\n"
            ),
            TestServerMode::HeadRejected => {
                "HTTP/1.1 405 Method Not Allowed\r\nConnection: close\r\n\r\n".to_string()
            }
        }
    }

    fn build_range_response(mode: TestServerMode, body: &[u8]) -> String {
        match mode {
            TestServerMode::RangeSupported | TestServerMode::HeadRejected => format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: 1\r\nContent-Range: bytes 0-0/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body[0] as char
            ),
            TestServerMode::RangeIgnored => build_full_response(body),
        }
    }

    fn build_full_response(body: &[u8]) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        )
    }
}
