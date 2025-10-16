use crate::config::DownloadConfig;
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::stats::DownloadStats;
use crate::status::DownloadStatus;
use crate::task::DownloadTask;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use log::{debug, info};
use reqwest::Client;
use std::fmt;
use std::path::PathBuf;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

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
    pub updated_at: Option<DateTime<Utc>>,
    pub status: Arc<tokio::sync::Mutex<DownloadStatus>>,
    pub stats: Arc<DownloadStats>,
}

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
        _: Option<DownloadStatus>,
        stats: Arc<DownloadStats>,
    ) -> Self {
        debug!(
            "[Worker {}] Created for URL: {}, range: {}-{}",
            id, url, start, end
        );
        Self {
            config,
            task,
            client,
            id,
            url,
            start,
            end,
            file_path,
            downloaded_size: AtomicU64::new(downloaded_size.unwrap_or(0)), // 从 request/task 初始化
            total_size: AtomicU64::new(end - start + 1),
            paused: Arc::new(AtomicBool::new(false)),
            canceled: Arc::new(AtomicBool::new(false)),
            deleted: Arc::new(AtomicBool::new(false)),
            updated_at: Some(Utc::now()),
            status: Arc::new(tokio::sync::Mutex::new(DownloadStatus::Pending)),
            stats,
        }
    }

    pub async fn start(&self) -> Result<(), DownloadError> {
        info!("[Worker {}] Starting download", self.id);

        if let Some(task_arc) = self.task.upgrade() {
            let _ = task_arc.worker_event_tx.send(DownloadEvent::Start(self.id));
        }

        *self.status.lock().await = DownloadStatus::Running;

        let mut retry_count = 0;
        let max_retries = self.config.retry.max_retries;
        let mut downloaded_size = 0u64;

        // 节流相关
        let throttle = &self.config.progress_throttle;
        let emit_interval = tokio::time::Duration::from_millis(throttle.interval_ms);
        let threshold: u64 = throttle.threshold_bytes;

        let mut last_emit = tokio::time::Instant::now();
        let mut last_reported: u64 = 0;

        loop {
            if self.deleted.load(Ordering::Relaxed) {
                debug!("[Worker {}] Download deleted", self.id);
                return Ok(());
            }

            if self.canceled.load(Ordering::Relaxed) {
                debug!("[Worker {}] Download canceled", self.id);
                return Ok(());
            }

            while self.paused.load(Ordering::Relaxed) {
                debug!("[Worker {}] Paused, waiting...", self.id);
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }

            let mut request = self.client.get(&self.url);
            let range_start = self.start + downloaded_size;
            if range_start <= self.end {
                request = request.header("Range", format!("bytes={}-{}", range_start, self.end));
            }

            let response = match request.send().await {
                Ok(res) => {
                    if !res.status().is_success() {
                        let err = DownloadError::HttpError(
                            self.id,
                            res.status().as_u16(),
                            res.status().to_string(),
                        );
                        debug!(
                            "[Worker {}] HTTP error: {}, retry_count: {}",
                            self.id,
                            res.status(),
                            retry_count
                        );

                        self.stats.record_failure();

                        if retry_count >= max_retries {
                            if let Some(task_arc) = self.task.upgrade() {
                                let _ = task_arc
                                    .worker_event_tx
                                    .send(DownloadEvent::Error(self.id, err.clone()));
                            }
                            *self.status.lock().await = DownloadStatus::Failed(err.clone());
                            return Err(err);
                        }
                        retry_count += 1;
                        tokio::time::sleep(tokio::time::Duration::from_secs(2u64.pow(retry_count)))
                            .await;

                        self.stats.record_retry();

                        continue;
                    }
                    res
                }
                Err(e) => {
                    debug!(
                        "[Worker {}] Network error: {:?}, retry_count: {}",
                        self.id, e, retry_count
                    );

                    self.stats.record_failure();

                    let err = DownloadError::NetworkError(self.id, e.to_string());
                    if retry_count >= max_retries {
                        if let Some(task_arc) = self.task.upgrade() {
                            let _ = task_arc.worker_event_tx.send(DownloadEvent::Error(
                                self.id,
                                DownloadError::Other(format!("{:?}", e)),
                            ));
                        }
                        *self.status.lock().await = DownloadStatus::Failed(err.clone());
                        return Err(err);
                    }
                    retry_count += 1;
                    tokio::time::sleep(tokio::time::Duration::from_secs(2u64.pow(retry_count)))
                        .await;

                    self.stats.record_retry();
                    continue;
                }
            };

            let content_length = response.content_length().unwrap_or(0);
            self.total_size.store(content_length, Ordering::Relaxed);

            debug!(
                "[Worker {}] Response received, content length: {}",
                self.id, content_length
            );

            let file_path = &*self.file_path;
            debug!("[Worker {}] Writing to file: {:?}", self.id, file_path);

            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(file_path)
                .await?;

            file.seek(tokio::io::SeekFrom::Start(range_start)).await?;

            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                if self.canceled.load(Ordering::Relaxed) {
                    debug!("[Worker {}] Download canceled during writing", self.id);
                    return Err(DownloadError::Canceled(self.id));
                }

                while self.paused.load(Ordering::Relaxed) {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }

                let chunk = chunk?;
                file.write_all(&chunk).await?;
                let chunk_len = chunk.len() as u64;
                downloaded_size += chunk_len;

                self.stats.update_worker(self.id, chunk_len).await;

                self.downloaded_size
                    .store(downloaded_size, Ordering::Relaxed);

                if downloaded_size - last_reported >= threshold
                    || last_emit.elapsed() >= emit_interval
                {
                    if let Some(task_arc) = self.task.upgrade() {
                        let _ = task_arc.worker_event_tx.send(DownloadEvent::Progress {
                            id: self.id,
                            downloaded: downloaded_size,
                            total: self.total_size.load(Ordering::Relaxed),
                        });
                    }
                    last_reported = downloaded_size;
                    last_emit = tokio::time::Instant::now();
                }
            }

            if let Some(task_arc) = self.task.upgrade() {
                let _ = task_arc.worker_event_tx.send(DownloadEvent::Progress {
                    id: self.id,
                    downloaded: downloaded_size,
                    total: self.total_size.load(Ordering::Relaxed),
                });
            }

            let expected_length = self.end - self.start + 1;
            let actual_length = self.downloaded_size.load(Ordering::Relaxed);
            if actual_length != expected_length {
                debug!(
                    "[Worker {}] Download length mismatch! expected: {}, actual: {}",
                    self.id, expected_length, actual_length
                );
                *self.status.lock().await = DownloadStatus::Failed(DownloadError::Other(
                    "Downloaded length mismatch".into(),
                ));
                return Err(DownloadError::Other(format!(
                    "Worker {} downloaded length mismatch",
                    self.id
                )));
            }

            debug!("[Worker {}] Finished downloading assigned range", self.id);

            break;
        }

        self.stats.record_success();

        *self.status.lock().await = DownloadStatus::Completed;
        info!("[Worker {}] Download completed successfully", self.id);

        if let Some(task_arc) = self.task.upgrade() {
            let _ = task_arc
                .worker_event_tx
                .send(DownloadEvent::Complete(self.id));
        }

        Ok(())
    }

    pub async fn pause(&self) -> Result<(), DownloadError> {
        self.paused.store(true, Ordering::Relaxed);
        *self.status.lock().await = DownloadStatus::Paused;
        debug!("[Worker {}] Paused", self.id);
        Ok(())
    }

    pub async fn resume(&self) -> Result<(), DownloadError> {
        self.paused.store(false, Ordering::Relaxed);
        *self.status.lock().await = DownloadStatus::Running;
        debug!("[Worker {}] Resumed", self.id);
        Ok(())
    }

    pub async fn cancel(&self) -> Result<(), DownloadError> {
        self.canceled.store(true, Ordering::Relaxed);
        *self.status.lock().await = DownloadStatus::Canceled;
        debug!("[Worker {}] Canceled", self.id);
        if let Some(task_arc) = self.task.upgrade() {
            let _ = task_arc
                .worker_event_tx
                .send(DownloadEvent::Cancel(self.id));
        }
        Ok(())
    }

    pub async fn delete(&self) -> Result<(), DownloadError> {
        self.deleted.store(true, Ordering::Relaxed);
        *self.status.lock().await = DownloadStatus::Deleted;
        debug!("[Worker {}] Deleted", self.id);
        if let Some(task_arc) = self.task.upgrade() {
            let _ = task_arc
                .worker_event_tx
                .send(DownloadEvent::Delete(self.id));
        }
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
