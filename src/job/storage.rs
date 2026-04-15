use crate::error::Error;
use crate::job::Task;
use chrono::Utc;
use log::debug;
use std::sync::Arc;

impl Task {
    pub async fn persist_task(self: &Arc<Self>) -> Result<(), Error> {
        {
            let mut updated_at_guard = self.updated_at.lock().await;
            *updated_at_guard = Some(Utc::now());
        }

        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Persisting task", self.id);
            if let Err(err) = persistence.save_task(self).await {
                debug!("[Task {}] Failed to persist task: {:?}", self.id, err);
            }
        }
        Ok(())
    }

    pub async fn persist_task_checksums(self: &Arc<Self>) -> Result<(), Error> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Persisting task checksums", self.id);
            let checksums = self.checksums.lock().await;
            if let Err(err) = persistence.save_checksums(&checksums, self.id).await {
                debug!(
                    "[Task {}] Failed to persist task checksums: {:?}",
                    self.id, err
                );
            }
        }
        Ok(())
    }

    pub async fn persist_task_worker(self: &Arc<Self>, worker_id: u32) -> Result<(), Error> {
        if let Some(persistence) = self.persistence.as_ref() {
            let worker_opt = {
                let workers = self.workers.read().await;
                workers
                    .iter()
                    .find(|worker| worker.id == worker_id)
                    .cloned()
            };

            if let Some(worker) = worker_opt {
                {
                    let mut updated_at_guard = worker.updated_at.lock().await;
                    *updated_at_guard = Some(Utc::now());
                }
                if let Err(err) = persistence.save_worker(&worker).await {
                    debug!(
                        "[Task {}] Failed to persist worker {}: {:?}",
                        self.id, worker_id, err
                    );
                }
            } else {
                debug!(
                    "[Task {}] Worker {} not found, cannot persist",
                    self.id, worker_id
                );
            }
        }
        Ok(())
    }

    pub(crate) fn persist_task_worker_later(self: &Arc<Self>, worker_id: u32) {
        let task = Arc::clone(self);
        tokio::spawn(async move {
            let _ = task.persist_task_worker(worker_id).await;
        });
    }

    pub(crate) async fn purge_task(self: &Arc<Self>) -> Result<(), Error> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Deleting task", self.id);
            if let Err(err) = persistence.delete_task(self.id).await {
                debug!("[Task {}] Failed to delete task: {:?}", self.id, err);
            }
            if let Err(err) = persistence.delete_workers(self.id).await {
                debug!("[Task {}] Failed to delete task worker: {:?}", self.id, err);
            }
        }
        Ok(())
    }

    pub(crate) async fn purge_task_workers(self: &Arc<Self>) -> Result<(), Error> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Deleting task workers", self.id);
            if let Err(err) = persistence.delete_workers(self.id).await {
                debug!("[Task {}] Failed to delete task worker: {:?}", self.id, err);
            }
        }
        Ok(())
    }

    pub(crate) async fn purge_task_checksums(self: &Arc<Self>) -> Result<(), Error> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Deleting task checksums", self.id);
            if let Err(err) = persistence.delete_checksums(self.id).await {
                debug!(
                    "[Task {}] Failed to delete task checksums: {:?}",
                    self.id, err
                );
            }
        }
        Ok(())
    }
}
