use super::repository::DownloadRepository;
use crate::DownloadError;
use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct MemoryRepository {
    tasks: Arc<RwLock<HashMap<u32, DBDownloadTask>>>,
    workers: Arc<RwLock<HashMap<u32, DBDownloadWorker>>>,
    checksums: Arc<RwLock<HashMap<u32, DBDownloadChecksum>>>,
}

impl MemoryRepository {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            workers: Arc::new(RwLock::new(HashMap::new())),
            checksums: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl DownloadRepository for MemoryRepository {
    // ---------------- Task ----------------
    async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, DownloadError> {
        Ok(self.tasks.read().await.values().cloned().collect())
    }

    async fn load_task(&self, task_id: u32) -> Result<Option<DBDownloadTask>, DownloadError> {
        Ok(self.tasks.read().await.get(&task_id).cloned())
    }

    async fn save_task(&self, task: &DBDownloadTask) -> Result<(), DownloadError> {
        self.tasks.write().await.insert(task.id, task.clone());
        Ok(())
    }

    async fn delete_task(&self, task_id: u32) -> Result<(), DownloadError> {
        self.tasks.write().await.remove(&task_id);

        // 删除关联 workers
        let mut workers = self.workers.write().await;
        workers.retain(|_, w| w.task_id != task_id);

        // 删除关联 checksums
        let mut checksums = self.checksums.write().await;
        checksums.retain(|_, c| c.task_id != task_id);

        Ok(())
    }

    // ---------------- Worker ----------------
    async fn load_workers(&self, task_id: u32) -> Result<Vec<DBDownloadWorker>, DownloadError> {
        Ok(self
            .workers
            .read()
            .await
            .values()
            .filter(|w| w.task_id == task_id)
            .cloned()
            .collect())
    }

    async fn load_worker(&self, worker_id: u32) -> Result<Option<DBDownloadWorker>, DownloadError> {
        Ok(self.workers.read().await.get(&worker_id).cloned())
    }

    async fn save_worker(&self, worker: &DBDownloadWorker) -> Result<(), DownloadError> {
        self.workers.write().await.insert(worker.id, worker.clone());
        Ok(())
    }

    async fn delete_workers(&self, task_id: u32) -> Result<(), DownloadError> {
        let mut workers = self.workers.write().await;
        workers.retain(|_, w| w.task_id != task_id);
        Ok(())
    }

    // ---------------- Checksum ----------------
    async fn load_checksums(&self, task_id: u32) -> Result<Vec<DBDownloadChecksum>, DownloadError> {
        Ok(self
            .checksums
            .read()
            .await
            .values()
            .filter(|c| c.task_id == task_id)
            .cloned()
            .collect())
    }

    async fn load_checksum(
        &self,
        checksum_id: u32,
    ) -> Result<Option<DBDownloadChecksum>, DownloadError> {
        Ok(self.checksums.read().await.get(&checksum_id).cloned())
    }

    async fn save_checksum(&self, checksum: &DBDownloadChecksum) -> Result<(), DownloadError> {
        self.checksums
            .write()
            .await
            .insert(checksum.id, checksum.clone());
        Ok(())
    }

    async fn delete_checksums(&self, task_id: u32) -> Result<(), DownloadError> {
        let mut checksums = self.checksums.write().await;
        checksums.retain(|_, c| c.task_id != task_id);
        Ok(())
    }
}
