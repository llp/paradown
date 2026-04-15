use crate::error::Error;
use crate::protocol_probe::parse_content_range;
use crate::runtime::apply_http_request_options;
use crate::transfer::driver::TransferDriver;
use crate::worker::Worker;
use crate::worker::transfer::ProgressReporter;
use async_trait::async_trait;
use futures_util::StreamExt;
use log::debug;
use reqwest::{StatusCode, header};
use std::sync::atomic::Ordering;

pub(crate) struct HttpTransferDriver;

#[async_trait]
impl TransferDriver for HttpTransferDriver {
    async fn build_request(
        &self,
        worker: &Worker,
        range_start: u64,
        use_range_requests: bool,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let task = worker.task.upgrade().ok_or_else(|| {
            Error::Other(format!("Worker {} lost its parent task", worker.id))
        })?;
        let mut request =
            apply_http_request_options(worker.client.get(worker.spec.locator()), task.http_request_options())?;
        if use_range_requests && range_start <= worker.end {
            request = request.header("Range", format!("bytes={}-{}", range_start, worker.end));

            if range_start > worker.start {
                if let Some(validator) = task.resume_validator().await {
                    request = request.header(header::IF_RANGE, validator);
                }
            }
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
            if expected_start > worker.start && response.status() == StatusCode::OK {
                return Err(Error::ResumeInvalidated(
                    worker.id,
                    "remote resource no longer matches stored validator".into(),
                ));
            }

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

    async fn stream_response(
        &self,
        worker: &Worker,
        response: reqwest::Response,
        downloaded_size: &mut u64,
        reporter: &mut ProgressReporter,
        use_range_requests: bool,
        range_start: u64,
    ) -> Result<(), Error> {
        let task = worker
            .task
            .upgrade()
            .ok_or_else(|| Error::Other(format!("Worker {} lost its parent task", worker.id)))?;
        let payload_store = task.payload_store().await?;
        let mut write_offset = if use_range_requests {
            range_start
        } else {
            worker.start
        };

        debug!(
            "[Worker {}] Writing locator {} at payload offset {}",
            worker.id,
            worker.spec.locator(),
            write_offset
        );

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
            payload_store.write_at(write_offset, &chunk).await?;
            write_offset = write_offset.saturating_add(chunk_len);
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
