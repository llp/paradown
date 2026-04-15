use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::status::DownloadStatus;
use crate::worker::DownloadWorker;
use crate::worker_retry::next_retry_delay;
use crate::worker_transfer::ProgressReporter;
use log::{debug, info};
use std::sync::atomic::Ordering;

impl DownloadWorker {
    pub async fn start(&self) -> Result<(), DownloadError> {
        if self.is_running.swap(true, Ordering::Relaxed) {
            debug!("[Worker {}] start() called, but already running", self.id);
            return Ok(());
        }

        info!("[Worker {}] Starting download loop", self.id);

        let run_result = self.run_download_loop().await;
        let final_result = match run_result {
            Ok(()) => Ok(()),
            Err(err) => {
                self.mark_failed(err.clone()).await;
                Err(err)
            }
        };

        self.is_running.store(false, Ordering::Relaxed);
        final_result
    }

    async fn run_download_loop(&self) -> Result<(), DownloadError> {
        if !self.prepare_run_state().await? {
            return Ok(());
        }

        let mut downloaded_size = self.downloaded_size.load(Ordering::Relaxed);
        let expected_length = self.expected_length();

        if downloaded_size >= expected_length {
            self.finish_success().await;
            return Ok(());
        }

        let mut retry_count = 0;
        let mut progress = ProgressReporter::new(self, downloaded_size);

        loop {
            if self.should_stop_gracefully() {
                return Ok(());
            }

            if !self.wait_until_resumed().await {
                return Ok(());
            }

            let use_range_requests = self.supports_range_requests();
            let range_start = self.start.saturating_add(downloaded_size);

            let response = match self
                .build_request(range_start, use_range_requests)
                .send()
                .await
            {
                Ok(response) => {
                    if let Err(protocol_err) =
                        self.validate_response(&response, use_range_requests, range_start)
                    {
                        let err = DownloadError::HttpError(
                            self.id,
                            response.status().as_u16(),
                            protocol_err.to_string(),
                        );
                        self.retry_or_fail(&mut retry_count, err).await?;
                        continue;
                    }

                    response
                }
                Err(err) => {
                    let err = DownloadError::NetworkError(self.id, err.to_string());
                    self.retry_or_fail(&mut retry_count, err).await?;
                    continue;
                }
            };

            let response_length =
                self.resolve_content_length(&response, use_range_requests, range_start);
            let total_size = if use_range_requests {
                downloaded_size.saturating_add(response_length)
            } else {
                response_length
            };
            self.total_size.store(total_size, Ordering::Relaxed);

            debug!(
                "[Worker {}] Response received, content length: {}",
                self.id, response_length
            );

            self.stream_response_to_file(
                response,
                &mut downloaded_size,
                &mut progress,
                use_range_requests,
                range_start,
            )
            .await?;

            if self.should_stop_gracefully() {
                return Ok(());
            }

            progress.flush(self, downloaded_size).await;
            self.validate_downloaded_length(downloaded_size, expected_length)?;

            debug!("[Worker {}] Finished downloading assigned range", self.id);
            break;
        }

        self.finish_success().await;
        Ok(())
    }

    async fn prepare_run_state(&self) -> Result<bool, DownloadError> {
        let current_status = self.status.lock().await.clone();
        match current_status {
            DownloadStatus::Paused => {
                debug!("[Worker {}] Resuming paused task", self.id);
                self.paused.store(true, Ordering::Relaxed);
            }
            DownloadStatus::Canceled | DownloadStatus::Deleted => {
                debug!(
                    "[Worker {}] Task cannot start, status: {:?}",
                    self.id, current_status
                );
                return Err(DownloadError::Other(format!(
                    "Task cannot start, status: {:?}",
                    current_status
                )));
            }
            DownloadStatus::Completed => {
                debug!(
                    "[Worker {}] Task already completed, skipping start",
                    self.id
                );
                return Ok(false);
            }
            _ => {}
        }

        self.set_status(DownloadStatus::Running).await;
        self.emit_worker_event(DownloadEvent::Start(self.id));
        Ok(true)
    }

    async fn finish_success(&self) {
        self.stats.record_success();
        self.set_status(DownloadStatus::Completed).await;
        info!("[Worker {}] Download completed successfully", self.id);
        self.emit_worker_event(DownloadEvent::Complete(self.id));
    }

    async fn mark_failed(&self, err: DownloadError) {
        debug!("[Worker {}] Marked as failed: {:?}", self.id, err);
        self.set_status(DownloadStatus::Failed(err.clone())).await;
        self.emit_worker_event(DownloadEvent::Error(self.id, err));
    }

    async fn retry_or_fail(
        &self,
        retry_count: &mut u32,
        err: DownloadError,
    ) -> Result<(), DownloadError> {
        debug!(
            "[Worker {}] Attempt failed: {:?}, retry_count: {}",
            self.id, err, retry_count
        );
        self.stats.record_failure();

        let Some(delay) = next_retry_delay(&self.config.retry, *retry_count) else {
            return Err(err);
        };

        *retry_count += 1;
        self.stats.record_retry();

        debug!(
            "[Worker {}] Retrying in {:?} (attempt {}/{})",
            self.id, delay, retry_count, self.config.retry.max_retries
        );
        tokio::time::sleep(delay).await;

        Ok(())
    }
}
