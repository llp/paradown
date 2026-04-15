use crate::config::DownloadConfig;
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::stats::DownloadStats;
use crate::status::DownloadStatus;
use crate::task::DownloadTask;
use chrono::{DateTime, Utc};
use log::debug;
use reqwest::Client;
use std::fmt;
use std::path::PathBuf;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::sync::Mutex;

pub struct DownloadWorker {
    pub task: Weak<DownloadTask>,
    pub config: Arc<DownloadConfig>,
    pub client: Arc<Client>,
    pub id: u32,
    pub url: String,
    pub start: u64,
    pub end: u64,
    pub downloaded_size: AtomicU64,
    pub total_size: AtomicU64,
    pub file_path: Arc<PathBuf>,
    pub paused: Arc<AtomicBool>,
    pub canceled: Arc<AtomicBool>,
    pub deleted: Arc<AtomicBool>,
    pub updated_at: Mutex<Option<DateTime<Utc>>>,
    pub status: Arc<tokio::sync::Mutex<DownloadStatus>>,
    pub stats: Arc<DownloadStats>,
    pub is_running: AtomicBool,
}

pub type SegmentWorker = DownloadWorker;

impl DownloadWorker {
    pub fn new(
        id: u32,
        config: Arc<DownloadConfig>,
        task: Weak<DownloadTask>,
        client: Arc<Client>,
        url: String,
        start: u64,
        end: u64,
        downloaded_size: Option<u64>,
        file_path: Arc<PathBuf>,
        status: Option<DownloadStatus>,
        stats: Arc<DownloadStats>,
        updated_at: Option<DateTime<Utc>>,
    ) -> Self {
        debug!(
            "[Worker {}] Created for URL: {}, range: {}-{}",
            id, url, start, end
        );

        let (paused, canceled, deleted) = match status {
            Some(DownloadStatus::Paused) => (true, false, false),
            Some(DownloadStatus::Canceled) => (false, true, false),
            Some(DownloadStatus::Deleted) => (false, false, true),
            _ => (false, false, false),
        };
        let now = Utc::now();

        Self {
            config,
            task,
            client,
            id,
            url,
            start,
            end,
            file_path,
            downloaded_size: AtomicU64::new(downloaded_size.unwrap_or(0)),
            total_size: AtomicU64::new(end - start + 1),
            paused: Arc::new(AtomicBool::new(paused)),
            canceled: Arc::new(AtomicBool::new(canceled)),
            deleted: Arc::new(AtomicBool::new(deleted)),
            updated_at: Mutex::new(Some(updated_at.unwrap_or(now))),
            status: Arc::new(tokio::sync::Mutex::new(
                status.unwrap_or(DownloadStatus::Pending),
            )),
            stats,
            is_running: AtomicBool::new(false),
        }
    }

    pub(crate) async fn set_status(&self, status: DownloadStatus) {
        *self.status.lock().await = status;
    }

    pub(crate) fn emit_worker_event(&self, event: DownloadEvent) {
        if let Some(task_arc) = self.task.upgrade() {
            let _ = task_arc.worker_event_tx.send(event);
        }
    }

    pub(crate) fn should_stop_gracefully(&self) -> bool {
        if self.deleted.load(Ordering::Relaxed) {
            debug!("[Worker {}] Download deleted", self.id);
            return true;
        }

        if self.canceled.load(Ordering::Relaxed) {
            debug!("[Worker {}] Download canceled", self.id);
            return true;
        }

        false
    }

    pub(crate) async fn wait_until_resumed(&self) -> bool {
        while self.paused.load(Ordering::Relaxed) {
            if self.should_stop_gracefully() {
                return false;
            }

            debug!("[Worker {}] Paused, waiting...", self.id);
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        true
    }

    pub async fn pause(&self) -> Result<(), DownloadError> {
        self.paused.store(true, Ordering::Relaxed);

        let should_emit = {
            let mut status_guard = self.status.lock().await;
            match *status_guard {
                DownloadStatus::Canceled
                | DownloadStatus::Failed(_)
                | DownloadStatus::Completed
                | DownloadStatus::Deleted => {
                    debug!(
                        "[Worker {}] Paused ignored — current status = {:?}",
                        self.id, *status_guard
                    );
                    return Ok(());
                }
                DownloadStatus::Paused => false,
                _ => {
                    *status_guard = DownloadStatus::Paused;
                    true
                }
            }
        };

        debug!("[Worker {}] Paused", self.id);
        if should_emit {
            self.emit_worker_event(DownloadEvent::Pause(self.id));
        }

        Ok(())
    }

    pub async fn resume(&self) -> Result<(), DownloadError> {
        let should_start = {
            let mut status_guard = self.status.lock().await;
            match *status_guard {
                DownloadStatus::Canceled
                | DownloadStatus::Failed(_)
                | DownloadStatus::Completed
                | DownloadStatus::Deleted => {
                    debug!(
                        "[Worker {}] Resume ignored — current status = {:?}",
                        self.id, *status_guard
                    );
                    return Ok(());
                }
                _ => {}
            }

            debug!("[Worker {}] Resumed", self.id);
            self.paused.store(false, Ordering::Relaxed);

            if self.is_running.load(Ordering::Relaxed) {
                *status_guard = DownloadStatus::Running;
                false
            } else {
                *status_guard = DownloadStatus::Running;
                true
            }
        };

        if should_start {
            self.start().await
        } else {
            Ok(())
        }
    }

    pub async fn cancel(&self) -> Result<(), DownloadError> {
        self.canceled.store(true, Ordering::Relaxed);
        self.set_status(DownloadStatus::Canceled).await;
        debug!("[Worker {}] Canceled", self.id);
        self.emit_worker_event(DownloadEvent::Cancel(self.id));
        Ok(())
    }

    pub async fn delete(&self) -> Result<(), DownloadError> {
        self.deleted.store(true, Ordering::Relaxed);
        self.set_status(DownloadStatus::Deleted).await;
        debug!("[Worker {}] Deleted", self.id);
        self.emit_worker_event(DownloadEvent::Delete(self.id));
        Ok(())
    }
}

impl fmt::Debug for DownloadWorker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadWorker")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("start", &self.start)
            .field("end", &self.end)
            .field("downloaded_size", &self.downloaded_size)
            .field("total_size", &self.total_size)
            .field("file_path", &self.file_path)
            .field("updated_at", &self.updated_at)
            .field("status", &self.status)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::DownloadWorker;
    use crate::config::DownloadConfig;
    use crate::stats::DownloadStats;
    use crate::status::DownloadStatus;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Weak};

    #[tokio::test]
    async fn resume_restarts_a_paused_worker_without_deadlocking() {
        let worker = DownloadWorker::new(
            1,
            Arc::new(DownloadConfig::default()),
            Weak::new(),
            Arc::new(reqwest::Client::builder().no_proxy().build().unwrap()),
            "https://example.com/file".into(),
            0,
            0,
            Some(1),
            Arc::new(PathBuf::from("/tmp/paradown-unused-worker-test")),
            Some(DownloadStatus::Paused),
            Arc::new(DownloadStats::new()),
            None,
        );

        worker.resume().await.unwrap();

        assert!(!worker.paused.load(Ordering::Relaxed));
        assert!(!worker.is_running.load(Ordering::Relaxed));
        assert!(matches!(
            &*worker.status.lock().await,
            DownloadStatus::Completed
        ));
    }
}
