use crate::chunk::plan_download_chunks;
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::job_finalize::finalize_download;
use crate::status::DownloadStatus;
use crate::task::DownloadTask;
use crate::worker::DownloadWorker;
use chrono::Utc;
use futures::future::join_all;
use log::{debug, warn};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::broadcast;

impl DownloadTask {
    pub(crate) fn spawn_worker_event_listener(self: &Arc<Self>) {
        let task = Arc::clone(self);
        let mut rx = self.worker_event_tx.subscribe();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => task.handle_worker_event(event).await,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "[Task {}] Worker event consumer lagged and skipped {} events",
                            task.id, skipped
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    pub(crate) async fn ensure_workers(
        self: &Arc<Self>,
        file_path: &Arc<PathBuf>,
    ) -> Result<Vec<Arc<DownloadWorker>>, DownloadError> {
        {
            let workers = self.workers.read().await;
            if !workers.is_empty() {
                return Ok(workers.clone());
            }
        }

        let created_workers = self.create_workers(file_path).await?;
        let mut workers = self.workers.write().await;
        if workers.is_empty() {
            *workers = created_workers.clone();
        }

        Ok(workers.clone())
    }

    pub(crate) fn spawn_workers(self: &Arc<Self>, workers: Vec<Arc<DownloadWorker>>) {
        let task = Arc::clone(self);
        tokio::spawn(async move {
            let mut worker_tasks = Vec::with_capacity(workers.len());
            for worker in workers {
                let worker = Arc::clone(&worker);
                worker_tasks.push(tokio::spawn(async move { worker.start().await }));
            }

            let results = join_all(worker_tasks).await;
            task.handle_worker_join_results(results).await;
        });
    }

    async fn create_workers(
        self: &Arc<Self>,
        file_path: &Arc<PathBuf>,
    ) -> Result<Vec<Arc<DownloadWorker>>, DownloadError> {
        let created_at = Utc::now();
        let chunks = plan_download_chunks(
            self.total_size.load(Ordering::Relaxed),
            self.config.worker_threads,
        );

        let mut workers = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let worker = Arc::new(DownloadWorker::new(
                chunk.index,
                Arc::clone(&self.config),
                Arc::downgrade(self),
                Arc::clone(&self.client),
                self.url.clone(),
                chunk.start,
                chunk.end,
                Some(0),
                Arc::clone(file_path),
                Some(DownloadStatus::Pending),
                Arc::clone(&self.stats),
                Some(created_at),
            ));

            if let Some(persistence) = self.persistence.as_ref() {
                let worker_clone = Arc::clone(&worker);
                if let Err(err) = persistence.save_worker(&worker_clone).await {
                    debug!(
                        "[Task {}] Failed to persist worker: {:?}",
                        worker_clone.id, err
                    );
                }
            }

            workers.push(worker);
        }

        Ok(workers)
    }

    async fn handle_worker_event(self: &Arc<Self>, event: DownloadEvent) {
        match event {
            DownloadEvent::Progress { id, .. } => self.handle_worker_progress(id).await,
            DownloadEvent::Complete(worker_id) => self.handle_worker_complete(worker_id).await,
            DownloadEvent::Error(worker_id, err) => self.handle_worker_error(worker_id, err).await,
            DownloadEvent::Pause(worker_id) => self.handle_worker_pause(worker_id).await,
            DownloadEvent::Cancel(worker_id) => self.handle_worker_cancel(worker_id).await,
            _ => {}
        }
    }

    async fn handle_worker_progress(self: &Arc<Self>, worker_id: u32) {
        self.persist_task_worker_later(worker_id);

        let total_downloaded = {
            let workers = self.workers.read().await;
            workers
                .iter()
                .map(|worker| worker.downloaded_size.load(Ordering::Relaxed))
                .sum()
        };

        self.downloaded_size
            .store(total_downloaded, Ordering::Relaxed);

        self.emit_manager_event(DownloadEvent::Progress {
            id: self.id,
            downloaded: total_downloaded,
            total: self.total_size.load(Ordering::Relaxed),
        });
    }

    async fn handle_worker_complete(self: &Arc<Self>, worker_id: u32) {
        debug!(
            "[Task {} Worker {}] Worker completed its download chunk",
            self.id, worker_id
        );

        self.persist_task_worker_later(worker_id);

        if !self.all_workers_completed().await {
            return;
        }

        debug!(
            "[Task {}] All workers finished, starting checksum verification",
            self.id
        );

        self.stats.snapshot().await;
        if let Err(err) = finalize_download(self).await {
            debug!("[Task {}] Finalization failed: {:?}", self.id, err);
            self.emit_manager_event(DownloadEvent::Error(self.id, err));
        }
    }

    async fn handle_worker_error(self: &Arc<Self>, worker_id: u32, err: DownloadError) {
        debug!("[Task {}] Worker error: {:?}", self.id, err);
        self.persist_task_worker_later(worker_id);
        self.stats.snapshot().await;

        let workers = { self.workers.read().await.clone() };
        for worker in workers {
            let _ = worker.cancel().await;
        }

        self.fail_with_error(err).await;
    }

    async fn handle_worker_pause(self: &Arc<Self>, worker_id: u32) {
        debug!("[Task {}] Task was paused", self.id);
        self.persist_task_worker_later(worker_id);
    }

    async fn handle_worker_cancel(self: &Arc<Self>, worker_id: u32) {
        debug!("[Task {}] Task was cancelled", self.id);
        self.persist_task_worker_later(worker_id);

        if !self.all_workers_canceled_or_completed().await {
            return;
        }

        self.set_status(DownloadStatus::Canceled).await;
        self.emit_manager_event(DownloadEvent::Cancel(self.id));
    }

    async fn handle_worker_join_results(
        self: &Arc<Self>,
        results: Vec<Result<Result<(), DownloadError>, tokio::task::JoinError>>,
    ) {
        for result in results {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    debug!("[Task {}] Worker failed: {:?}", self.id, err);
                    self.fail_with_error(err).await;
                }
                Err(err) => {
                    let join_error = DownloadError::Other(format!("{:?}", err));
                    debug!("[Task {}] Worker panicked: {:?}", self.id, err);
                    self.fail_with_error(join_error).await;
                }
            }
        }
    }

    async fn all_workers_completed(&self) -> bool {
        let workers = { self.workers.read().await.clone() };
        for worker in workers {
            let status = worker.status.lock().await;
            if !matches!(*status, DownloadStatus::Completed) {
                debug!(
                    "[Task {}] Worker not completed -> id: {}, status: {:?}",
                    self.id, worker.id, *status
                );
                return false;
            }
        }
        true
    }

    async fn all_workers_canceled_or_completed(&self) -> bool {
        let workers = { self.workers.read().await.clone() };
        for worker in workers {
            let status = worker.status.lock().await;
            if !matches!(
                *status,
                DownloadStatus::Canceled | DownloadStatus::Completed
            ) {
                return false;
            }
        }
        true
    }
}
