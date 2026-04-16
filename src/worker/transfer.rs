use crate::error::Error;
use crate::events::Event;
use crate::worker::Worker;
use std::sync::atomic::Ordering;
use tokio::time::{Duration, Instant};

pub(crate) struct ProgressReporter {
    emit_interval: Duration,
    threshold: u64,
    last_emit: Instant,
    last_reported: u64,
}

impl ProgressReporter {
    pub(crate) fn new(worker: &Worker, downloaded_size: u64) -> Self {
        let throttle = &worker.config.progress_throttle;
        Self {
            emit_interval: Duration::from_millis(throttle.interval_ms),
            threshold: throttle.threshold_bytes,
            last_emit: Instant::now(),
            last_reported: downloaded_size,
        }
    }

    pub(crate) async fn maybe_emit(&mut self, worker: &Worker, downloaded_size: u64) {
        let delta = downloaded_size.saturating_sub(self.last_reported);
        if delta < self.threshold && self.last_emit.elapsed() < self.emit_interval {
            return;
        }

        worker.emit_progress(downloaded_size);
        self.last_reported = downloaded_size;
        self.last_emit = Instant::now();
    }

    pub(crate) async fn flush(&mut self, worker: &Worker, downloaded_size: u64) {
        worker.emit_progress(downloaded_size);
        self.last_reported = downloaded_size;
        self.last_emit = Instant::now();
    }
}

impl Worker {
    pub(crate) fn expected_length(&self) -> u64 {
        if !self.length_known {
            return 0;
        }
        self.end.saturating_sub(self.start).saturating_add(1)
    }

    pub(crate) fn supports_range_requests(&self) -> bool {
        self.task
            .upgrade()
            .map(|task| task.supports_range_requests())
            .unwrap_or(false)
    }

    pub(crate) async fn acquire_rate_limit(&self, bytes: u64) {
        let Some(task) = self.task.upgrade() else {
            return;
        };
        let Some(manager) = task.manager.upgrade() else {
            return;
        };

        manager.rate_limiter.acquire(bytes).await;
    }

    pub(crate) fn emit_progress(&self, downloaded_size: u64) {
        self.emit_worker_event(Event::Progress {
            id: self.id,
            downloaded: downloaded_size,
            total: self.total_size.load(Ordering::Relaxed),
        });
    }

    pub(crate) fn validate_downloaded_length(
        &self,
        actual_length: u64,
        expected_length: u64,
    ) -> Result<(), Error> {
        if !self.length_known {
            return Ok(());
        }

        if actual_length != expected_length {
            return Err(Error::Other(format!(
                "Worker {} downloaded length mismatch: expected {}, got {}",
                self.id, expected_length, actual_length
            )));
        }

        Ok(())
    }
}
