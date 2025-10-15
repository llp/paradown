use crate::checksum::{ChecksumAlgorithm, DownloadChecksum};
use crate::config::DownloadConfig;
use crate::error::DownloadError;
use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
use crate::repository::{DownloadRepository, MemoryRepository, SqliteRepository};
use crate::task::DownloadTask;
use crate::worker::DownloadWorker;

use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PersistenceType {
    Memory,
    JsonFile(String),
    Sqlite(String),
}

pub struct DownloadPersistenceManager {
    repository: Arc<dyn DownloadRepository>,
    config: Arc<DownloadConfig>,
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

    /// 保存单个 Task，包括它的 workers 和 checksums
    pub async fn save_task(&self, task: &Arc<DownloadTask>) -> Result<(), DownloadError> {
        let db_task = self.task_to_db(task).await;
        self.repository.save_task(&db_task).await
    }

    /// 保存多个 Task
    pub async fn save_tasks(&self, tasks: &[Arc<DownloadTask>]) -> Result<(), DownloadError> {
        for task in tasks {
            self.save_task(task).await?;
        }
        Ok(())
    }

    /// 加载单个 Task
    pub async fn load_task(&self, task_id: u32) -> Result<Option<DBDownloadTask>, DownloadError> {
        self.repository.load_task(task_id).await
    }

    /// 加载所有 Task
    pub async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, DownloadError> {
        self.repository.load_tasks().await
    }

    /// 删除 Task
    pub async fn delete_task(&self, task_id: u32) -> Result<(), DownloadError> {
        self.repository.delete_task(task_id).await
    }
    //------------------------------------------------------------------------------------------------------

    /// 保存单个 Worker
    pub async fn save_worker(&self, worker: &Arc<DownloadWorker>) -> Result<(), DownloadError> {
        let db_worker = self.worker_to_db(worker).await;
        self.repository.save_worker(&db_worker).await
    }

    /// 批量保存 Worker
    pub async fn save_workers(&self, workers: &[Arc<DownloadWorker>]) -> Result<(), DownloadError> {
        for w in workers {
            self.save_worker(w).await?;
        }
        Ok(())
    }

    /// 加载指定 Task 的所有 Workers
    pub async fn load_workers(&self, task_id: u32) -> Result<Vec<DBDownloadWorker>, DownloadError> {
        self.repository.load_workers(task_id).await
    }

    /// 加载单个 Worker
    pub async fn load_worker(
        &self,
        worker_id: u32,
    ) -> Result<Option<DBDownloadWorker>, DownloadError> {
        self.repository.load_worker(worker_id).await
    }

    /// 删除指定 Task 的所有 Workers
    pub async fn delete_workers(&self, task_id: u32) -> Result<(), DownloadError> {
        self.repository.delete_workers(task_id).await
    }

    //------------------------------------------------------------------------------------------------------
    /// 加载指定 Task 的所有 Checksums
    pub async fn load_checksums(
        &self,
        task_id: u32,
    ) -> Result<Vec<DBDownloadChecksum>, DownloadError> {
        self.repository.load_checksums(task_id).await
    }

    /// 加载单个 Checksum（根据 ID）
    pub async fn load_checksum(
        &self,
        checksum_id: u32,
    ) -> Result<Option<DBDownloadChecksum>, DownloadError> {
        self.repository.load_checksum(checksum_id).await
    }

    /// 保存 Checksum
    pub async fn save_checksums(
        &self,
        checksums: &[DownloadChecksum],
        task_id: u32,
    ) -> Result<(), DownloadError> {
        for checksum in checksums {
            let db_checksum = self.checksum_to_db(checksum, task_id);
            self.repository.save_checksum(&db_checksum).await?
        }
        Ok(())
    }

    /// 删除指定 Task 的所有 Checksums
    pub async fn delete_checksums(&self, task_id: u32) -> Result<(), DownloadError> {
        self.repository.delete_checksums(task_id).await
    }

    //------------------------------------------------------------------------------------------

    /// 转换 Task -> DBDownloadTask
    async fn task_to_db(&self, task: &Arc<DownloadTask>) -> DBDownloadTask {
        let file_path_str = match task.file_path.get() {
            Some(p) => p.to_string_lossy().to_string(),
            None => "".to_string(),
        };

        // 从 OnceCell 获取 file_name，如果没设置就用空字符串
        let file_name = task.file_name.get().cloned().unwrap_or_default();

        let download_task = DBDownloadTask {
            id: task.id,
            url: task.url.clone(),
            file_name,
            file_path: file_path_str,
            status: task.status.lock().await.to_string(),
            downloaded_size: task.progress.load(Ordering::Relaxed),
            total_size: Some(task.total_size.load(Ordering::Relaxed)),
            created_at: task.created_at,
            updated_at: task.updated_at,
        };
        download_task
    }

    /// 转换 Worker -> DBDownloadWorker
    async fn worker_to_db(&self, worker: &Arc<DownloadWorker>) -> DBDownloadWorker {
        DBDownloadWorker {
            id: worker.id,
            task_id: worker.task.upgrade().map(|t| t.id).unwrap_or_default(),
            index: worker.id,
            start: worker.start,
            end: worker.end,
            downloaded: worker.downloaded.load(Ordering::Relaxed),
            status: worker.status.lock().await.to_string(),
            updated_at: worker.updated_at,
        }
    }

    /// 转换 Checksum -> DBDownloadChecksum
    fn checksum_to_db(&self, checksum: &DownloadChecksum, task_id: u32) -> DBDownloadChecksum {
        DBDownloadChecksum {
            id: 0,
            task_id,
            algorithm: match checksum.algorithm {
                ChecksumAlgorithm::MD5 => "MD5".to_string(),
                ChecksumAlgorithm::SHA1 => "SHA1".to_string(),
                ChecksumAlgorithm::SHA256 => "SHA256".to_string(),
                ChecksumAlgorithm::NONE => "NONE".to_string(),
            },
            value: checksum.value.clone().unwrap_or_default(),
            verified: false,
            verified_at: None,
        }
    }

    /// DBDownloadChecksum -> DownloadChecksum
    pub fn db_to_checksum(&self, model: &DBDownloadChecksum) -> DownloadChecksum {
        DownloadChecksum {
            algorithm: match model.algorithm.as_str() {
                "MD5" => ChecksumAlgorithm::MD5,
                "SHA1" => ChecksumAlgorithm::SHA1,
                "SHA256" => ChecksumAlgorithm::SHA256,
                _ => ChecksumAlgorithm::NONE,
            },
            value: Some(model.value.clone()),
        }
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
