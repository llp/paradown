use crate::diagnostics::write_failure_diagnostic;
use crate::error::Error;
use crate::events::Event;
use crate::job::Task;
use crate::status::Status;
use log::{debug, warn};
use std::io::ErrorKind;
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

    pub(crate) async fn stop_workers_and_fail(self: &Arc<Self>, err: Error) {
        self.set_status(Status::Failed(err.clone())).await;

        let workers = { self.workers.read().await.clone() };
        for worker in workers {
            let _ = worker.cancel().await;
        }

        debug!(
            "[Task {}] Stopped workers before surfacing failure: {:?}",
            self.id, err
        );
        self.fail_with_error(err).await;
    }

    pub(crate) async fn release_permit(&self) {
        let mut permit = self.permit.lock().await;
        if permit.take().is_some() {
            debug!("[Task {}] Released semaphore permit", self.id);
        }
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
        self.release_permit().await;
        self.clear_task_workers().await?;
        self.clear_manifest_state().await;

        self.delete_task_file().await?;
        self.purge_task_workers().await?;
        self.purge_task_checksums().await?;
        self.purge_task().await?;

        self.set_status(Status::Deleted).await;
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
            match tokio::fs::remove_file(file_path).await {
                Ok(_) => debug!("[Task {}] Removed file: {:?}", self.id, file_path),
                Err(err) if err.kind() == ErrorKind::NotFound => {
                    debug!("[Task {}] File not found: {:?}", self.id, file_path);
                }
                Err(err) => {
                    return Err(err.into());
                }
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

#[cfg(test)]
mod tests {
    use super::Task;
    use crate::config::Config;
    use crate::domain::{DownloadSpec, HttpRequestOptions};
    use crate::error::Error;
    use tempfile::TempDir;

    #[tokio::test]
    async fn delete_task_file_surfaces_non_file_errors() {
        let sandbox = TempDir::new().unwrap();
        let directory_path = sandbox.path().join("payload-dir");
        tokio::fs::create_dir_all(&directory_path).await.unwrap();

        let task = Task::new(
            1,
            DownloadSpec::parse("https://example.com/file.bin").unwrap(),
            Some("file.bin".into()),
            Some(directory_path.to_string_lossy().to_string()),
            None,
            HttpRequestOptions::default(),
            None,
            None,
            None,
            None,
            None,
            None,
            Vec::new(),
            std::sync::Arc::new(reqwest::Client::builder().no_proxy().build().unwrap()),
            std::sync::Arc::new(Config::default()),
            None,
            std::sync::Weak::new(),
            None,
            None,
        )
        .unwrap();

        let err = task
            .delete_task_file()
            .await
            .expect_err("removing a directory path should surface an error");
        assert!(matches!(err, Error::Io(_)), "unexpected error: {err}");
    }
}
