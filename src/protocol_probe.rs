use crate::domain::{HttpRequestOptions, HttpResourceIdentity};
use crate::error::Error;
use crate::runtime::apply_http_request_options;
use reqwest::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, ETAG, LAST_MODIFIED, RANGE,
};
use reqwest::{Client, Response, StatusCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DownloadProtocolProbe {
    pub total_size: u64,
    pub supports_range_requests: bool,
    pub resource_identity: HttpResourceIdentity,
    pub suggested_file_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentRange {
    pub start: u64,
    pub end: u64,
    pub total_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HeadProbe {
    total_size: Option<u64>,
    accepts_ranges: bool,
    resource_identity: HttpResourceIdentity,
    suggested_file_name: Option<String>,
}

pub(crate) async fn probe_download_target(
    client: &Client,
    url: &str,
    request_options: &HttpRequestOptions,
) -> Result<DownloadProtocolProbe, Error> {
    let head_probe = probe_with_head(client, url, request_options).await?;

    if let Some(ref head) = head_probe
        && head.total_size == Some(0)
    {
        return Ok(DownloadProtocolProbe {
            total_size: 0,
            supports_range_requests: head.accepts_ranges,
            resource_identity: head.resource_identity.clone(),
            suggested_file_name: head.suggested_file_name.clone(),
        });
    }

    let response =
        apply_http_request_options(client.get(url).header(RANGE, "bytes=0-0"), request_options)?
            .send()
            .await?;
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

async fn probe_with_head(
    client: &Client,
    url: &str,
    request_options: &HttpRequestOptions,
) -> Result<Option<HeadProbe>, Error> {
    let response = match apply_http_request_options(client.head(url), request_options)?
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };

    if !response.status().is_success() {
        return Ok(None);
    }

    Ok(Some(HeadProbe {
        total_size: parse_content_length(response.headers().get(CONTENT_LENGTH))?,
        accepts_ranges: header_accepts_ranges(response.headers().get(ACCEPT_RANGES)),
        resource_identity: extract_resource_identity(&response)?,
        suggested_file_name: extract_content_disposition_file_name(&response)?,
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
            let resource_identity = merge_resource_identity(
                head_probe.as_ref().map(|probe| &probe.resource_identity),
                &extract_resource_identity(&response)?,
            );
            let suggested_file_name =
                extract_content_disposition_file_name(&response)?.or_else(|| {
                    head_probe
                        .as_ref()
                        .and_then(|probe| probe.suggested_file_name.clone())
                });

            Ok(DownloadProtocolProbe {
                total_size,
                supports_range_requests: true,
                resource_identity,
                suggested_file_name,
            })
        }
        StatusCode::OK => {
            let total_size = parse_content_length(response.headers().get(CONTENT_LENGTH))?
                .or_else(|| head_probe.as_ref().and_then(|probe| probe.total_size))
                .ok_or_else(|| {
                    Error::Other(
                        "HTTP downloads without Content-Length are not supported yet".into(),
                    )
                })?;
            let resource_identity = merge_resource_identity(
                head_probe.as_ref().map(|probe| &probe.resource_identity),
                &extract_resource_identity(&response)?,
            );
            let suggested_file_name =
                extract_content_disposition_file_name(&response)?.or_else(|| {
                    head_probe
                        .as_ref()
                        .and_then(|probe| probe.suggested_file_name.clone())
                });

            Ok(DownloadProtocolProbe {
                total_size,
                supports_range_requests: false,
                resource_identity,
                suggested_file_name,
            })
        }
        StatusCode::RANGE_NOT_SATISFIABLE => {
            let total_size = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_unsatisfied_content_range)
                .or_else(|| head_probe.as_ref().and_then(|probe| probe.total_size))
                .unwrap_or(0);
            let resource_identity = merge_resource_identity(
                head_probe.as_ref().map(|probe| &probe.resource_identity),
                &extract_resource_identity(&response)?,
            );
            let suggested_file_name =
                extract_content_disposition_file_name(&response)?.or_else(|| {
                    head_probe
                        .as_ref()
                        .and_then(|probe| probe.suggested_file_name.clone())
                });

            Ok(DownloadProtocolProbe {
                total_size,
                supports_range_requests: total_size > 0,
                resource_identity,
                suggested_file_name,
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

fn extract_resource_identity(response: &Response) -> Result<HttpResourceIdentity, Error> {
    Ok(HttpResourceIdentity {
        resolved_url: Some(response.url().to_string()),
        entity_tag: response
            .headers()
            .get(ETAG)
            .map(|value| value.to_str().map(|value| value.to_string()))
            .transpose()?,
        last_modified: response
            .headers()
            .get(LAST_MODIFIED)
            .map(|value| value.to_str().map(|value| value.to_string()))
            .transpose()?,
    })
}

fn merge_resource_identity(
    head: Option<&HttpResourceIdentity>,
    response: &HttpResourceIdentity,
) -> HttpResourceIdentity {
    HttpResourceIdentity {
        resolved_url: response
            .resolved_url
            .clone()
            .or_else(|| head.and_then(|probe| probe.resolved_url.clone())),
        entity_tag: response
            .entity_tag
            .clone()
            .or_else(|| head.and_then(|probe| probe.entity_tag.clone())),
        last_modified: response
            .last_modified
            .clone()
            .or_else(|| head.and_then(|probe| probe.last_modified.clone())),
    }
}

fn extract_content_disposition_file_name(response: &Response) -> Result<Option<String>, Error> {
    let Some(value) = response.headers().get(CONTENT_DISPOSITION) else {
        return Ok(None);
    };

    parse_content_disposition_header(value)
}

fn parse_content_disposition_header(
    value: &reqwest::header::HeaderValue,
) -> Result<Option<String>, Error> {
    let value = value.to_str()?;
    for segment in value.split(';').skip(1) {
        let segment = segment.trim();
        if let Some(filename) = segment.strip_prefix("filename=") {
            return Ok(normalize_content_disposition_filename(filename));
        }
    }

    Ok(None)
}

fn normalize_content_disposition_filename(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"');
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
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
    use crate::domain::HttpRequestOptions;
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

        let probe = probe_download_target(&client, &server.url(), &HttpRequestOptions::default())
            .await
            .unwrap();
        assert_eq!(probe.total_size, 5);
        assert!(probe.supports_range_requests);
    }

    #[tokio::test]
    async fn falls_back_to_single_stream_when_range_is_ignored() {
        let server = TestHttpServer::spawn(TestServerMode::RangeIgnored).await;
        let client = Client::new();

        let probe = probe_download_target(&client, &server.url(), &HttpRequestOptions::default())
            .await
            .unwrap();
        assert_eq!(probe.total_size, 5);
        assert!(!probe.supports_range_requests);
    }

    #[tokio::test]
    async fn handles_head_rejection_with_range_probe() {
        let server = TestHttpServer::spawn(TestServerMode::HeadRejected).await;
        let client = Client::new();

        let probe = probe_download_target(&client, &server.url(), &HttpRequestOptions::default())
            .await
            .unwrap();
        assert_eq!(probe.total_size, 5);
        assert!(probe.supports_range_requests);
    }

    #[tokio::test]
    async fn rejects_targets_without_content_length() {
        let server = TestHttpServer::spawn(TestServerMode::NoContentLength).await;
        let client = Client::new();

        let err = probe_download_target(&client, &server.url(), &HttpRequestOptions::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("HTTP downloads without Content-Length are not supported yet")
        );
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
                            build_full_response(mode, &body)
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
        NoContentLength,
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
            TestServerMode::NoContentLength => {
                "HTTP/1.1 200 OK\r\nAccept-Ranges: none\r\nConnection: close\r\n\r\n".to_string()
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
            TestServerMode::RangeIgnored | TestServerMode::NoContentLength => {
                build_full_response(mode, body)
            }
        }
    }

    fn build_full_response(mode: TestServerMode, body: &[u8]) -> String {
        match mode {
            TestServerMode::NoContentLength => format!(
                "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{}",
                String::from_utf8_lossy(body)
            ),
            _ => format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                String::from_utf8_lossy(body)
            ),
        }
    }
}
