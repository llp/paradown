use crate::diagnostics::write_failure_diagnostic;
use crate::error::Error;
use crate::events::Event;
use crate::job::Task;
use crate::status::Status;
use log::{debug, warn};
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) enum StartDirective {
    Continue,
}

impl Task {
    pub(crate) async fn prepare_for_start(self: &Arc<Self>) -> Result<StartDirective, Error> {
        let status = self.status.lock().await;
        match &*status {
            Status::Pending => {
                debug!(
                    "[Task {}] Task is in Pending state, will attempt to start/restart the task",
                    self.id
                );
                Ok(StartDirective::Continue)
            }
            Status::Preparing | Status::Running => {
                debug!(
                    "[Task {}] Task already active: {:?}, skipping start",
                    self.id, *status
                );
                Err(Error::Other(format!(
                    "Task is {:?}, cannot start again",
                    *status
                )))
            }
            Status::Paused => {
                debug!(
                    "[Task {}] Paused task will restart preparation before resuming",
                    self.id
                );
                Ok(StartDirective::Continue)
            }
            Status::Completed => {
                debug!("[Task {}] Task already completed, skipping start", self.id);
                Err(Error::Other("Task already completed".into()))
            }
            Status::Canceled => {
                warn!(
                    "[Task {}] Task was previously canceled — restarting as new download",
                    self.id
                );
                drop(status);
                self.reset_task().await?;
                Ok(StartDirective::Continue)
            }
            Status::Failed(_) => {
                warn!(
                    "[Task {}] Task failed previously — resetting and restarting download",
                    self.id
                );
                drop(status);
                self.reset_task().await?;
                Ok(StartDirective::Continue)
            }
            Status::Deleted => {
                debug!("[Task {}] Task has been deleted, cannot start", self.id);
                Err(Error::Other("Task deleted".into()))
            }
        }
    }

    pub(crate) async fn set_status(&self, next_status: Status) {
        let mut status = self.status.lock().await;
        *status = next_status;
    }

    pub(crate) async fn fail_with_error(&self, err: Error) {
        self.set_status(Status::Failed(err.clone())).await;
        if let Err(diag_err) = write_failure_diagnostic(self, &err).await {
            warn!(
                "[Task {}] Failed to write failure diagnostic: {}",
                self.id, diag_err
            );
        }
        debug!("[Task {}] Marked as failed: {:?}", self.id, err);
        self.emit_manager_event(Event::Error(self.id, err));
    }

    pub async fn pause(self: &Arc<Self>) -> Result<(), Error> {
        let mut status = self.status.lock().await;
        match *status {
            Status::Running | Status::Preparing => {
                *status = Status::Paused;
                debug!("[Task {}] Paused", self.id);
            }
            Status::Paused => {
                debug!("[Task {}] Task is already paused", self.id);
                return Ok(());
            }
            Status::Pending
            | Status::Completed
            | Status::Canceled
            | Status::Deleted
            | Status::Failed(_) => {
                return Err(Error::Other(format!(
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
        self.emit_manager_event(Event::Pause(self.id));
        Ok(())
    }

    pub async fn resume(self: &Arc<Self>) -> Result<(), Error> {
        let should_restart = {
            let mut status = self.status.lock().await;
            match *status {
                Status::Pending | Status::Running => {
                    drop(status);
                    return Box::pin(self.start()).await;
                }
                Status::Paused => {
                    debug!(
                        "[Task {}] Resuming paused task through fresh preparation",
                        self.id
                    );
                    *status = Status::Pending;
                    true
                }
                Status::Completed
                | Status::Preparing
                | Status::Canceled
                | Status::Deleted
                | Status::Failed(_) => {
                    return Err(Error::Other(format!(
                        "Cannot resume task in state: {:?}",
                        *status
                    )));
                }
            }
        };

        if should_restart {
            self.persist_task().await?;
            return Box::pin(self.start()).await;
        }

        Ok(())
    }

    pub async fn cancel(self: &Arc<Self>) -> Result<(), Error> {
        let mut status = self.status.lock().await;
        match *status {
            Status::Pending | Status::Preparing | Status::Running | Status::Paused => {
                *status = Status::Canceled;
                debug!("[Task {}] Canceled", self.id);
            }
            Status::Completed | Status::Canceled | Status::Deleted | Status::Failed(_) => {
                return Err(Error::Other(format!(
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

        self.emit_manager_event(Event::Cancel(self.id));
        Ok(())
    }

    pub async fn delete(self: &Arc<Self>) -> Result<(), Error> {
        let workers = { self.workers.read().await.clone() };
        for worker in workers {
            let _ = worker.delete().await;
        }
        self.clear_task_workers().await?;
        self.clear_manifest_state().await;

        self.delete_task_file().await?;
        self.purge_task_workers().await?;
        self.purge_task_checksums().await?;
        self.purge_task().await?;

        self.emit_manager_event(Event::Delete(self.id));
        Ok(())
    }

    pub(crate) async fn reset_task(self: &Arc<Self>) -> Result<(), Error> {
        self.clear_task_workers().await?;
        self.purge_task_workers().await?;

        self.downloaded_size.store(0, Ordering::Relaxed);
        self.total_size.store(0, Ordering::Relaxed);
        self.total_size_known.store(false, Ordering::Relaxed);
        self.range_requests_supported
            .store(false, Ordering::Relaxed);
        self.protocol_probe_completed
            .store(false, Ordering::Relaxed);
        self.clear_manifest_state().await;

        self.delete_task_file().await?;
        Ok(())
    }

    pub(crate) async fn delete_task_file(&self) -> Result<(), Error> {
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

    pub(crate) async fn clear_task_workers(&self) -> Result<(), Error> {
        let mut workers = self.workers.write().await;
        workers.clear();
        Ok(())
    }
}
