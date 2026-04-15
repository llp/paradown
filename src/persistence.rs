use crate::checksum::DownloadChecksum;
use crate::config::DownloadConfig;
use crate::error::DownloadError;
use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
use crate::repository::{DownloadRepository, MemoryRepository, SqliteRepository};
use crate::storage_mapping::{checksum_to_db, task_to_db, worker_to_db};
use crate::task::DownloadTask;
use crate::worker::DownloadWorker;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PersistenceType {
    Memory,
    JsonFile(String),
    Sqlite(PathBuf),
}

pub type StorageBackend = PersistenceType;

pub struct DownloadPersistenceManager {
    repository: Arc<dyn DownloadRepository>,
    config: Arc<DownloadConfig>,
}

pub type DownloadStore = DownloadPersistenceManager;

#[derive(Debug, Clone)]
pub struct StoredDownloadBundle {
    pub task: DBDownloadTask,
    pub workers: Vec<DBDownloadWorker>,
    pub checksums: Vec<DBDownloadChecksum>,
}

impl DownloadPersistenceManager {
    pub async fn new(config: Arc<DownloadConfig>) -> Result<Self, DownloadError> {
        let repository: Arc<dyn DownloadRepository> = match &config.persistence_type {
            PersistenceType::Memory => Arc::new(MemoryRepository::new()),
            PersistenceType::JsonFile(path) => {
                return Err(DownloadError::ConfigError(format!(
                    "JsonFile persistence not implemented yet: {}",
                    path
                )));
            }
            PersistenceType::Sqlite(path) => Arc::new(SqliteRepository::new(path).await?),
        };

        Ok(Self { repository, config })
    }

    pub fn repository(&self) -> Arc<dyn DownloadRepository> {
        Arc::clone(&self.repository)
    }

    pub async fn save_task(&self, task: &Arc<DownloadTask>) -> Result<(), DownloadError> {
        let db_task = task_to_db(task).await;
        self.repository.save_task(&db_task).await
    }

    pub async fn save_tasks(&self, tasks: &[Arc<DownloadTask>]) -> Result<(), DownloadError> {
        for task in tasks {
            self.save_task(task).await?;
        }
        Ok(())
    }

    pub async fn load_task(&self, task_id: u32) -> Result<Option<DBDownloadTask>, DownloadError> {
        self.repository.load_task(task_id).await
    }

    pub async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, DownloadError> {
        self.repository.load_tasks().await
    }

    pub async fn load_task_bundles(&self) -> Result<Vec<StoredDownloadBundle>, DownloadError> {
        let tasks = self.load_tasks().await?;
        let mut bundles = Vec::with_capacity(tasks.len());

        for task in tasks {
            let task_id = task.id;
            let workers = self.load_workers(task_id).await?;
            let checksums = self.load_checksums(task_id).await?;
            bundles.push(StoredDownloadBundle {
                task,
                workers,
                checksums,
            });
        }

        Ok(bundles)
    }

    pub async fn delete_task(&self, task_id: u32) -> Result<(), DownloadError> {
        self.repository.delete_task(task_id).await
    }

    pub async fn save_worker(&self, worker: &Arc<DownloadWorker>) -> Result<(), DownloadError> {
        let db_worker = worker_to_db(worker).await;
        self.repository.save_worker(&db_worker).await
    }

    pub async fn save_workers(&self, workers: &[Arc<DownloadWorker>]) -> Result<(), DownloadError> {
        for worker in workers {
            self.save_worker(worker).await?;
        }
        Ok(())
    }

    pub async fn load_workers(&self, task_id: u32) -> Result<Vec<DBDownloadWorker>, DownloadError> {
        self.repository.load_workers(task_id).await
    }

    pub async fn load_worker(
        &self,
        worker_id: u32,
    ) -> Result<Option<DBDownloadWorker>, DownloadError> {
        self.repository.load_worker(worker_id).await
    }

    pub async fn delete_workers(&self, task_id: u32) -> Result<(), DownloadError> {
        self.repository.delete_workers(task_id).await
    }

    pub async fn load_checksums(
        &self,
        task_id: u32,
    ) -> Result<Vec<DBDownloadChecksum>, DownloadError> {
        self.repository.load_checksums(task_id).await
    }

    pub async fn load_checksum(
        &self,
        checksum_id: u32,
    ) -> Result<Option<DBDownloadChecksum>, DownloadError> {
        self.repository.load_checksum(checksum_id).await
    }

    pub async fn save_checksums(
        &self,
        checksums: &[DownloadChecksum],
        task_id: u32,
    ) -> Result<(), DownloadError> {
        for checksum in checksums {
            let db_checksum = checksum_to_db(checksum, task_id);
            self.repository.save_checksum(&db_checksum).await?;
        }
        Ok(())
    }

    pub async fn delete_checksums(&self, task_id: u32) -> Result<(), DownloadError> {
        self.repository.delete_checksums(task_id).await
    }
}

impl Clone for DownloadPersistenceManager {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            config: Arc::clone(&self.config),
        }
    }
}
