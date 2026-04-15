use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::protocol_probe::parse_content_range;
use crate::worker::DownloadWorker;
use futures_util::StreamExt;
use log::debug;
use reqwest::{StatusCode, header};
use std::sync::atomic::Ordering;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::time::{Duration, Instant};

pub(crate) struct ProgressReporter {
    emit_interval: Duration,
    threshold: u64,
    last_emit: Instant,
    last_reported: u64,
}

impl ProgressReporter {
    pub(crate) fn new(worker: &DownloadWorker, downloaded_size: u64) -> Self {
        let throttle = &worker.config.progress_throttle;
        Self {
            emit_interval: Duration::from_millis(throttle.interval_ms),
            threshold: throttle.threshold_bytes,
            last_emit: Instant::now(),
            last_reported: downloaded_size,
        }
    }

    pub(crate) async fn maybe_emit(&mut self, worker: &DownloadWorker, downloaded_size: u64) {
        let delta = downloaded_size.saturating_sub(self.last_reported);
        if delta < self.threshold && self.last_emit.elapsed() < self.emit_interval {
            return;
        }

        worker.emit_progress(downloaded_size);
        self.last_reported = downloaded_size;
        self.last_emit = Instant::now();
    }

    pub(crate) async fn flush(&mut self, worker: &DownloadWorker, downloaded_size: u64) {
        worker.emit_progress(downloaded_size);
        self.last_reported = downloaded_size;
        self.last_emit = Instant::now();
    }
}

impl DownloadWorker {
    pub(crate) fn expected_length(&self) -> u64 {
        self.end.saturating_sub(self.start).saturating_add(1)
    }

    pub(crate) fn supports_range_requests(&self) -> bool {
        self.task
            .upgrade()
            .map(|task| task.supports_range_requests())
            .unwrap_or(false)
    }

    pub(crate) fn build_request(
        &self,
        range_start: u64,
        use_range_requests: bool,
    ) -> reqwest::RequestBuilder {
        let mut request = self.client.get(&self.url);
        if use_range_requests && range_start <= self.end {
            request = request.header("Range", format!("bytes={}-{}", range_start, self.end));
        }
        request
    }

    pub(crate) fn resolve_content_length(
        &self,
        response: &reqwest::Response,
        use_range_requests: bool,
        range_start: u64,
    ) -> u64 {
        response.content_length().unwrap_or_else(|| {
            if use_range_requests {
                self.end.saturating_sub(range_start).saturating_add(1)
            } else {
                self.expected_length()
            }
        })
    }

    pub(crate) async fn stream_response_to_file(
        &self,
        response: reqwest::Response,
        downloaded_size: &mut u64,
        reporter: &mut ProgressReporter,
        use_range_requests: bool,
        range_start: u64,
    ) -> Result<(), DownloadError> {
        let file_path = &*self.file_path;
        debug!("[Worker {}] Writing to file: {:?}", self.id, file_path);

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(file_path)
            .await?;

        let file_offset = if use_range_requests {
            range_start
        } else {
            self.start
        };
        file.seek(tokio::io::SeekFrom::Start(file_offset)).await?;

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if self.should_stop_gracefully() {
                return Ok(());
            }

            if !self.wait_until_resumed().await {
                return Ok(());
            }

            let chunk = chunk?;
            let chunk_len = chunk.len() as u64;
            file.write_all(&chunk).await?;
            *downloaded_size += chunk_len;

            self.stats.update_worker(self.id, chunk_len).await;
            self.downloaded_size
                .store(*downloaded_size, Ordering::Relaxed);

            reporter.maybe_emit(self, *downloaded_size).await;
        }

        Ok(())
    }

    pub(crate) fn emit_progress(&self, downloaded_size: u64) {
        self.emit_worker_event(DownloadEvent::Progress {
            id: self.id,
            downloaded: downloaded_size,
            total: self.total_size.load(Ordering::Relaxed),
        });
    }

    pub(crate) fn validate_downloaded_length(
        &self,
        actual_length: u64,
        expected_length: u64,
    ) -> Result<(), DownloadError> {
        if actual_length != expected_length {
            return Err(DownloadError::Other(format!(
                "Worker {} downloaded length mismatch: expected {}, got {}",
                self.id, expected_length, actual_length
            )));
        }

        Ok(())
    }

    pub(crate) fn validate_response(
        &self,
        response: &reqwest::Response,
        use_range_requests: bool,
        expected_start: u64,
    ) -> Result<(), DownloadError> {
        if use_range_requests {
            if response.status() != StatusCode::PARTIAL_CONTENT {
                return Err(DownloadError::Other(format!(
                    "Expected 206 Partial Content, got {}",
                    response.status()
                )));
            }

            let content_range = response
                .headers()
                .get(header::CONTENT_RANGE)
                .ok_or_else(|| DownloadError::Other("Missing Content-Range".into()))?
                .to_str()?;
            let content_range = parse_content_range(content_range)
                .ok_or_else(|| DownloadError::Other("Invalid Content-Range".into()))?;

            if content_range.start != expected_start || content_range.end != self.end {
                return Err(DownloadError::Other(format!(
                    "Unexpected Content-Range {}-{} for expected {}-{}",
                    content_range.start, content_range.end, expected_start, self.end
                )));
            }

            return Ok(());
        }

        if response.status() != StatusCode::OK {
            return Err(DownloadError::Other(format!(
                "Expected 200 OK, got {}",
                response.status()
            )));
        }

        Ok(())
    }
}
