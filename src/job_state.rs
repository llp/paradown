use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::status::DownloadStatus;
use crate::task::DownloadTask;
use log::{debug, warn};
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) enum StartDirective {
    Continue,
    Resume,
}

impl DownloadTask {
    pub(crate) async fn prepare_for_start(
        self: &Arc<Self>,
    ) -> Result<StartDirective, DownloadError> {
        let status = self.status.lock().await;
        match &*status {
            DownloadStatus::Pending => {
                debug!(
                    "[Task {}] Task is in Pending state, will attempt to start/restart the task",
                    self.id
                );
                Ok(StartDirective::Continue)
            }
            DownloadStatus::Preparing | DownloadStatus::Running => {
                debug!(
                    "[Task {}] Task already active: {:?}, skipping start",
                    self.id, *status
                );
                Err(DownloadError::Other(format!(
                    "Task is {:?}, cannot start again",
                    *status
                )))
            }
            DownloadStatus::Paused => {
                if self.protocol_probe_completed() {
                    debug!("[Task {}] Resuming paused task", self.id);
                    Ok(StartDirective::Resume)
                } else {
                    debug!(
                        "[Task {}] Paused task has not been probed in this process, restarting preparation",
                        self.id
                    );
                    Ok(StartDirective::Continue)
                }
            }
            DownloadStatus::Completed => {
                debug!("[Task {}] Task already completed, skipping start", self.id);
                Err(DownloadError::Other("Task already completed".into()))
            }
            DownloadStatus::Canceled => {
                warn!(
                    "[Task {}] Task was previously canceled — restarting as new download",
                    self.id
                );
                drop(status);
                self.reset_task().await?;
                Ok(StartDirective::Continue)
            }
            DownloadStatus::Failed(_) => {
                warn!(
                    "[Task {}] Task failed previously — resetting and restarting download",
                    self.id
                );
                drop(status);
                self.reset_task().await?;
                Ok(StartDirective::Continue)
            }
            DownloadStatus::Deleted => {
                debug!("[Task {}] Task has been deleted, cannot start", self.id);
                Err(DownloadError::Other("Task deleted".into()))
            }
        }
    }

    pub(crate) async fn set_status(&self, next_status: DownloadStatus) {
        let mut status = self.status.lock().await;
        *status = next_status;
    }

    pub(crate) async fn fail_with_error(&self, err: DownloadError) {
        self.set_status(DownloadStatus::Failed(err.clone())).await;
        debug!("[Task {}] Marked as failed: {:?}", self.id, err);
        self.emit_manager_event(DownloadEvent::Error(self.id, err));
    }

    pub async fn pause(self: &Arc<Self>) -> Result<(), DownloadError> {
        let mut status = self.status.lock().await;
        match *status {
            DownloadStatus::Running | DownloadStatus::Preparing => {
                *status = DownloadStatus::Paused;
                debug!("[Task {}] Paused", self.id);
            }
            DownloadStatus::Paused => {
                debug!("[Task {}] Task is already paused", self.id);
                return Ok(());
            }
            DownloadStatus::Pending
            | DownloadStatus::Completed
            | DownloadStatus::Canceled
            | DownloadStatus::Deleted
            | DownloadStatus::Failed(_) => {
                return Err(DownloadError::Other(format!(
                    "Cannot pause task in state: {:?}",
                    *status
                )));
            }
        }
        drop(status);

        let workers = { self.workers.read().await.clone() };
        for worker in workers {
            let _ = worker.pause().await;
        }

        self.persist_task().await?;
        self.emit_manager_event(DownloadEvent::Pause(self.id));
        Ok(())
    }

    pub async fn resume(self: &Arc<Self>) -> Result<(), DownloadError> {
        let should_resume_workers = {
            let mut status = self.status.lock().await;
            match *status {
                DownloadStatus::Pending | DownloadStatus::Running => {
                    drop(status);
                    return Box::pin(self.start()).await;
                }
                DownloadStatus::Paused => {
                    if !self.protocol_probe_completed() {
                        debug!(
                            "[Task {}] Paused task has no protocol probe state, restarting through start()",
                            self.id
                        );
                        *status = DownloadStatus::Pending;
                        drop(status);
                        return Box::pin(self.start()).await;
                    }

                    if self.total_size.load(Ordering::Relaxed) == 0 {
                        debug!(
                            "[Task {}] Resuming paused task in Preparing/Pending phase",
                            self.id
                        );
                        *status = DownloadStatus::Preparing;
                        false
                    } else {
                        debug!("[Task {}] Resuming paused task", self.id);
                        *status = DownloadStatus::Running;
                        true
                    }
                }
                DownloadStatus::Completed
                | DownloadStatus::Preparing
                | DownloadStatus::Canceled
                | DownloadStatus::Deleted
                | DownloadStatus::Failed(_) => {
                    return Err(DownloadError::Other(format!(
                        "Cannot resume task in state: {:?}",
                        *status
                    )));
                }
            }
        };

        self.persist_task().await?;

        if should_resume_workers {
            let workers = { self.workers.read().await.clone() };
            for worker in workers {
                let _ = worker.resume().await;
            }
            self.emit_manager_event(DownloadEvent::Start(self.id));
        }

        Ok(())
    }

    pub async fn cancel(self: &Arc<Self>) -> Result<(), DownloadError> {
        let mut status = self.status.lock().await;
        match *status {
            DownloadStatus::Pending
            | DownloadStatus::Preparing
            | DownloadStatus::Running
            | DownloadStatus::Paused => {
                *status = DownloadStatus::Canceled;
                debug!("[Task {}] Canceled", self.id);
            }
            DownloadStatus::Completed
            | DownloadStatus::Canceled
            | DownloadStatus::Deleted
            | DownloadStatus::Failed(_) => {
                return Err(DownloadError::Other(format!(
                    "Cannot cancel task in state: {:?}",
                    *status
                )));
            }
        }
        drop(status);

        self.persist_task().await?;

        let workers = { self.workers.read().await.clone() };
        for worker in workers {
            let _ = worker.cancel().await;
        }

        self.emit_manager_event(DownloadEvent::Cancel(self.id));
        Ok(())
    }

    pub async fn delete(self: &Arc<Self>) -> Result<(), DownloadError> {
        let workers = { self.workers.read().await.clone() };
        for worker in workers {
            let _ = worker.delete().await;
        }
        self.clear_task_workers().await?;

        self.delete_task_file().await?;
        self.purge_task_workers().await?;
        self.purge_task_checksums().await?;
        self.purge_task().await?;

        self.emit_manager_event(DownloadEvent::Delete(self.id));
        Ok(())
    }

    pub(crate) async fn reset_task(self: &Arc<Self>) -> Result<(), DownloadError> {
        self.clear_task_workers().await?;
        self.purge_task_workers().await?;

        self.downloaded_size.store(0, Ordering::Relaxed);
        self.total_size.store(0, Ordering::Relaxed);
        self.range_requests_supported
            .store(false, Ordering::Relaxed);
        self.protocol_probe_completed
            .store(false, Ordering::Relaxed);

        self.delete_task_file().await?;
        Ok(())
    }

    pub(crate) async fn delete_task_file(&self) -> Result<(), DownloadError> {
        if let Some(file_path) = self.file_path.get() {
            if file_path.exists() {
                match tokio::fs::remove_file(file_path).await {
                    Ok(_) => debug!("[Task {}] Removed file: {:?}", self.id, file_path),
                    Err(err) => debug!(
                        "[Task {}] Failed to remove file {:?}: {}",
                        self.id, file_path, err
                    ),
                }
            } else {
                debug!("[Task {}] File not found: {:?}", self.id, file_path);
            }
        }
        Ok(())
    }

    pub(crate) async fn clear_task_workers(&self) -> Result<(), DownloadError> {
        let mut workers = self.workers.write().await;
        workers.clear();
        Ok(())
    }
}
