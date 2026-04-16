pub(crate) mod mapping;

use self::mapping::{
    block_states_to_db, checksum_to_db, piece_states_to_db, task_to_db, worker_to_db,
};
use crate::checksum::Checksum;
use crate::config::Config;
use crate::domain::{BlockState, PieceState};
use crate::error::Error;
use crate::job::Task;
use crate::repository::models::{
    DBDownloadBlock, DBDownloadChecksum, DBDownloadPiece, DBDownloadTask, DBDownloadWorker,
};
use crate::repository::{MemoryRepository, Repository, SqliteRepository};
use crate::worker::Worker;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Backend {
    Memory,
    JsonFile(String),
    Sqlite(PathBuf),
}

pub struct Store {
    repository: Arc<dyn Repository>,
    config: Arc<Config>,
}

#[derive(Debug, Clone)]
pub struct StoredBundle {
    pub task: DBDownloadTask,
    pub workers: Vec<DBDownloadWorker>,
    pub pieces: Vec<DBDownloadPiece>,
    pub blocks: Vec<DBDownloadBlock>,
    pub checksums: Vec<DBDownloadChecksum>,
}

impl Store {
    pub async fn new(config: Arc<Config>) -> Result<Self, Error> {
        let repository: Arc<dyn Repository> = match &config.storage_backend {
            Backend::Memory => Arc::new(MemoryRepository::new()),
            Backend::JsonFile(path) => {
                return Err(Error::ConfigError(format!(
                    "JsonFile persistence not implemented yet: {}",
                    path
                )));
            }
            Backend::Sqlite(path) => Arc::new(SqliteRepository::new(path).await?),
        };

        Ok(Self { repository, config })
    }

    pub fn repository(&self) -> Arc<dyn Repository> {
        Arc::clone(&self.repository)
    }

    pub async fn save_task(&self, task: &Arc<Task>) -> Result<(), Error> {
        let db_task = task_to_db(task).await;
        self.repository.save_task(&db_task).await?;
        let piece_states = task.piece_states_snapshot().await;
        self.save_piece_states(task.id, &piece_states).await?;
        let block_states = task.block_states_snapshot().await;
        self.save_block_states(task.id, &block_states).await
    }

    pub async fn save_tasks(&self, tasks: &[Arc<Task>]) -> Result<(), Error> {
        for task in tasks {
            self.save_task(task).await?;
        }
        Ok(())
    }

    pub async fn load_task(&self, task_id: u32) -> Result<Option<DBDownloadTask>, Error> {
        self.repository.load_task(task_id).await
    }

    pub async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, Error> {
        self.repository.load_tasks().await
    }

    pub async fn load_task_bundles(&self) -> Result<Vec<StoredBundle>, Error> {
        let tasks = self.load_tasks().await?;
        let mut bundles = Vec::with_capacity(tasks.len());

        for task in tasks {
            let task_id = task.id;
            let workers = self.load_workers(task_id).await?;
            let pieces = self.load_pieces(task_id).await?;
            let blocks = self.load_blocks(task_id).await?;
            let checksums = self.load_checksums(task_id).await?;
            bundles.push(StoredBundle {
                task,
                workers,
                pieces,
                blocks,
                checksums,
            });
        }

        Ok(bundles)
    }

    pub async fn delete_task(&self, task_id: u32) -> Result<(), Error> {
        self.repository.delete_task(task_id).await?;
        self.repository.delete_pieces(task_id).await?;
        self.repository.delete_blocks(task_id).await
    }

    pub async fn save_worker(&self, worker: &Arc<Worker>) -> Result<(), Error> {
        let db_worker = worker_to_db(worker).await;
        self.repository.save_worker(&db_worker).await
    }

    pub async fn save_workers(&self, workers: &[Arc<Worker>]) -> Result<(), Error> {
        for worker in workers {
            self.save_worker(worker).await?;
        }
        Ok(())
    }

    pub async fn load_workers(&self, task_id: u32) -> Result<Vec<DBDownloadWorker>, Error> {
        self.repository.load_workers(task_id).await
    }

    pub async fn load_worker(&self, worker_id: u32) -> Result<Option<DBDownloadWorker>, Error> {
        self.repository.load_worker(worker_id).await
    }

    pub async fn delete_workers(&self, task_id: u32) -> Result<(), Error> {
        self.repository.delete_workers(task_id).await
    }

    pub async fn load_pieces(&self, task_id: u32) -> Result<Vec<DBDownloadPiece>, Error> {
        self.repository.load_pieces(task_id).await
    }

    pub async fn save_piece_states(
        &self,
        task_id: u32,
        piece_states: &[PieceState],
    ) -> Result<(), Error> {
        let db_pieces = piece_states_to_db(task_id, piece_states);
        self.repository.save_pieces(task_id, &db_pieces).await
    }

    pub async fn delete_pieces(&self, task_id: u32) -> Result<(), Error> {
        self.repository.delete_pieces(task_id).await
    }

    pub async fn load_blocks(&self, task_id: u32) -> Result<Vec<DBDownloadBlock>, Error> {
        self.repository.load_blocks(task_id).await
    }

    pub async fn save_block_states(
        &self,
        task_id: u32,
        block_states: &[BlockState],
    ) -> Result<(), Error> {
        let db_blocks = block_states_to_db(task_id, block_states);
        self.repository.save_blocks(task_id, &db_blocks).await
    }

    pub async fn delete_blocks(&self, task_id: u32) -> Result<(), Error> {
        self.repository.delete_blocks(task_id).await
    }

    pub async fn load_checksums(&self, task_id: u32) -> Result<Vec<DBDownloadChecksum>, Error> {
        self.repository.load_checksums(task_id).await
    }

    pub async fn load_checksum(
        &self,
        checksum_id: u32,
    ) -> Result<Option<DBDownloadChecksum>, Error> {
        self.repository.load_checksum(checksum_id).await
    }

    pub async fn save_checksums(&self, checksums: &[Checksum], task_id: u32) -> Result<(), Error> {
        for checksum in checksums {
            let db_checksum = checksum_to_db(checksum, task_id);
            self.repository.save_checksum(&db_checksum).await?;
        }
        Ok(())
    }

    pub async fn delete_checksums(&self, task_id: u32) -> Result<(), Error> {
        self.repository.delete_checksums(task_id).await
    }
}

impl Clone for Store {
    fn clone(&self) -> Self {
        Self {
            repository: Arc::clone(&self.repository),
            config: Arc::clone(&self.config),
        }
    }
}
