use crate::error::Error;
use crate::protocol_probe::parse_content_range;
use crate::transfer::driver::TransferDriver;
use crate::worker::Worker;
use crate::worker::transfer::ProgressReporter;
use async_trait::async_trait;
use futures_util::StreamExt;
use log::debug;
use reqwest::{StatusCode, header};
use std::sync::atomic::Ordering;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

pub(crate) struct HttpTransferDriver;

#[async_trait]
impl TransferDriver for HttpTransferDriver {
    fn build_request(
        &self,
        worker: &Worker,
        range_start: u64,
        use_range_requests: bool,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let mut request = worker.client.get(worker.spec.locator());
        if use_range_requests && range_start <= worker.end {
            request = request.header("Range", format!("bytes={}-{}", range_start, worker.end));
        }
        Ok(request)
    }

    fn resolve_content_length(
        &self,
        worker: &Worker,
        response: &reqwest::Response,
        use_range_requests: bool,
        range_start: u64,
    ) -> u64 {
        response.content_length().unwrap_or_else(|| {
            if use_range_requests {
                worker.end.saturating_sub(range_start).saturating_add(1)
            } else {
                worker.expected_length()
            }
        })
    }

    fn validate_response(
        &self,
        worker: &Worker,
        response: &reqwest::Response,
        use_range_requests: bool,
        expected_start: u64,
    ) -> Result<(), Error> {
        if use_range_requests {
            if response.status() != StatusCode::PARTIAL_CONTENT {
                return Err(Error::Other(format!(
                    "Expected 206 Partial Content, got {}",
                    response.status()
                )));
            }

            let content_range = response
                .headers()
                .get(header::CONTENT_RANGE)
                .ok_or_else(|| Error::Other("Missing Content-Range".into()))?
                .to_str()?;
            let content_range = parse_content_range(content_range)
                .ok_or_else(|| Error::Other("Invalid Content-Range".into()))?;

            if content_range.start != expected_start || content_range.end != worker.end {
                return Err(Error::Other(format!(
                    "Unexpected Content-Range {}-{} for expected {}-{}",
                    content_range.start, content_range.end, expected_start, worker.end
                )));
            }

            return Ok(());
        }

        if response.status() != StatusCode::OK {
            return Err(Error::Other(format!(
                "Expected 200 OK, got {}",
                response.status()
            )));
        }

        Ok(())
    }

    async fn stream_response_to_file(
        &self,
        worker: &Worker,
        response: reqwest::Response,
        downloaded_size: &mut u64,
        reporter: &mut ProgressReporter,
        use_range_requests: bool,
        range_start: u64,
    ) -> Result<(), Error> {
        let file_path = &*worker.file_path;
        debug!("[Worker {}] Writing to file: {:?}", worker.id, file_path);

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(file_path)
            .await?;

        let file_offset = if use_range_requests {
            range_start
        } else {
            worker.start
        };
        file.seek(tokio::io::SeekFrom::Start(file_offset)).await?;

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if worker.should_stop_gracefully() {
                return Ok(());
            }

            if !worker.wait_until_resumed().await {
                return Ok(());
            }

            let chunk = chunk.map_err(|err| Error::NetworkError(worker.id, err.to_string()))?;
            let chunk_len = chunk.len() as u64;
            worker.acquire_rate_limit(chunk_len).await;
            file.write_all(&chunk).await?;
            *downloaded_size += chunk_len;

            worker.stats.update_worker(worker.id, chunk_len).await;
            worker
                .downloaded_size
                .store(*downloaded_size, Ordering::Relaxed);

            reporter.maybe_emit(worker, *downloaded_size).await;
        }

        Ok(())
    }
}
