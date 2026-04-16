use super::contract::Repository;
use crate::Error;
use crate::repository::models::{
    DBDownloadBlock, DBDownloadChecksum, DBDownloadPiece, DBDownloadTask, DBDownloadWorker,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct JsonRepositoryState {
    tasks: Vec<DBDownloadTask>,
    workers: Vec<DBDownloadWorker>,
    pieces: Vec<DBDownloadPiece>,
    blocks: Vec<DBDownloadBlock>,
    checksums: Vec<DBDownloadChecksum>,
}

pub struct JsonFileRepository {
    path: PathBuf,
    state: Arc<RwLock<JsonRepositoryState>>,
}

impl JsonFileRepository {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let state = match tokio::fs::read_to_string(&path).await {
            Ok(contents) if !contents.trim().is_empty() => serde_json::from_str(&contents)
                .map_err(|err| {
                    Error::Other(format!("failed to parse {}: {err}", path.display()))
                })?,
            Ok(_) => JsonRepositoryState::default(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                JsonRepositoryState::default()
            }
            Err(err) => return Err(Error::Io(err.to_string())),
        };

        Ok(Self {
            path,
            state: Arc::new(RwLock::new(state)),
        })
    }

    async fn persist_state(&self) -> Result<(), Error> {
        let snapshot = self.state.read().await.clone();
        let payload = serde_json::to_vec_pretty(&snapshot)
            .map_err(|err| Error::Other(format!("failed to serialize repository state: {err}")))?;
        tokio::fs::write(&self.path, payload).await?;
        Ok(())
    }
}

#[async_trait]
impl Repository for JsonFileRepository {
    async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, Error> {
        let mut tasks = self.state.read().await.tasks.clone();
        tasks.sort_by_key(|task| task.id);
        Ok(tasks)
    }

    async fn load_task(&self, task_id: u32) -> Result<Option<DBDownloadTask>, Error> {
        Ok(self
            .state
            .read()
            .await
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .cloned())
    }

    async fn save_task(&self, task: &DBDownloadTask) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.tasks.retain(|stored| stored.id != task.id);
            state.tasks.push(task.clone());
        }
        self.persist_state().await
    }

    async fn delete_task(&self, task_id: u32) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.tasks.retain(|task| task.id != task_id);
            state.workers.retain(|worker| worker.task_id != task_id);
            state.pieces.retain(|piece| piece.task_id != task_id);
            state.blocks.retain(|block| block.task_id != task_id);
            state
                .checksums
                .retain(|checksum| checksum.task_id != task_id);
        }
        self.persist_state().await
    }

    async fn save_bundle(
        &self,
        task: &DBDownloadTask,
        workers: &[DBDownloadWorker],
        pieces: &[DBDownloadPiece],
        blocks: &[DBDownloadBlock],
        checksums: &[DBDownloadChecksum],
    ) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.tasks.retain(|stored| stored.id != task.id);
            state.tasks.push(task.clone());
            state.workers.retain(|worker| worker.task_id != task.id);
            state.workers.extend(workers.iter().cloned());
            state.pieces.retain(|piece| piece.task_id != task.id);
            state.pieces.extend(pieces.iter().cloned());
            state.blocks.retain(|block| block.task_id != task.id);
            state.blocks.extend(blocks.iter().cloned());
            state
                .checksums
                .retain(|checksum| checksum.task_id != task.id);
            state.checksums.extend(checksums.iter().cloned());
        }
        self.persist_state().await
    }

    async fn load_workers(&self, task_id: u32) -> Result<Vec<DBDownloadWorker>, Error> {
        let mut workers: Vec<_> = self
            .state
            .read()
            .await
            .workers
            .iter()
            .filter(|worker| worker.task_id == task_id)
            .cloned()
            .collect();
        workers.sort_by_key(|worker| worker.index);
        Ok(workers)
    }

    async fn load_worker(&self, worker_id: u32) -> Result<Option<DBDownloadWorker>, Error> {
        Ok(self
            .state
            .read()
            .await
            .workers
            .iter()
            .find(|worker| worker.id == worker_id)
            .cloned())
    }

    async fn save_worker(&self, worker: &DBDownloadWorker) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.workers.retain(|stored| {
                !(stored.task_id == worker.task_id && stored.index == worker.index)
            });
            state.workers.push(worker.clone());
        }
        self.persist_state().await
    }

    async fn delete_workers(&self, task_id: u32) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.workers.retain(|worker| worker.task_id != task_id);
        }
        self.persist_state().await
    }

    async fn load_pieces(&self, task_id: u32) -> Result<Vec<DBDownloadPiece>, Error> {
        let mut pieces: Vec<_> = self
            .state
            .read()
            .await
            .pieces
            .iter()
            .filter(|piece| piece.task_id == task_id)
            .cloned()
            .collect();
        pieces.sort_by_key(|piece| piece.piece_index);
        Ok(pieces)
    }

    async fn save_pieces(&self, task_id: u32, pieces: &[DBDownloadPiece]) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.pieces.retain(|piece| piece.task_id != task_id);
            state.pieces.extend(pieces.iter().cloned());
        }
        self.persist_state().await
    }

    async fn delete_pieces(&self, task_id: u32) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.pieces.retain(|piece| piece.task_id != task_id);
        }
        self.persist_state().await
    }

    async fn load_blocks(&self, task_id: u32) -> Result<Vec<DBDownloadBlock>, Error> {
        let mut blocks: Vec<_> = self
            .state
            .read()
            .await
            .blocks
            .iter()
            .filter(|block| block.task_id == task_id)
            .cloned()
            .collect();
        blocks.sort_by_key(|block| (block.piece_index, block.block_index));
        Ok(blocks)
    }

    async fn save_blocks(&self, task_id: u32, blocks: &[DBDownloadBlock]) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.blocks.retain(|block| block.task_id != task_id);
            state.blocks.extend(blocks.iter().cloned());
        }
        self.persist_state().await
    }

    async fn delete_blocks(&self, task_id: u32) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.blocks.retain(|block| block.task_id != task_id);
        }
        self.persist_state().await
    }

    async fn load_checksums(&self, task_id: u32) -> Result<Vec<DBDownloadChecksum>, Error> {
        let mut checksums: Vec<_> = self
            .state
            .read()
            .await
            .checksums
            .iter()
            .filter(|checksum| checksum.task_id == task_id)
            .cloned()
            .collect();
        checksums.sort_by(|left, right| left.algorithm.cmp(&right.algorithm));
        Ok(checksums)
    }

    async fn load_checksum(&self, checksum_id: u32) -> Result<Option<DBDownloadChecksum>, Error> {
        Ok(self
            .state
            .read()
            .await
            .checksums
            .iter()
            .find(|checksum| checksum.id == checksum_id)
            .cloned())
    }

    async fn save_checksum(&self, checksum: &DBDownloadChecksum) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state.checksums.retain(|stored| {
                !(stored.task_id == checksum.task_id && stored.algorithm == checksum.algorithm)
            });
            state.checksums.push(checksum.clone());
        }
        self.persist_state().await
    }

    async fn delete_checksums(&self, task_id: u32) -> Result<(), Error> {
        {
            let mut state = self.state.write().await;
            state
                .checksums
                .retain(|checksum| checksum.task_id != task_id);
        }
        self.persist_state().await
    }
}

#[cfg(test)]
mod tests {
    use super::JsonFileRepository;
    use crate::repository::contract::Repository;
    use crate::repository::models::{DBDownloadTask, DBDownloadWorker};
    use tempfile::tempdir;

    #[tokio::test]
    async fn persists_task_bundle_to_json_file() {
        let sandbox = tempdir().unwrap();
        let path = sandbox.path().join("state.json");
        let repository = JsonFileRepository::new(&path).await.unwrap();

        repository
            .save_task(&DBDownloadTask {
                id: 9,
                url: "https://example.com/file.bin".into(),
                spec_json: "{}".into(),
                source_set_json: "{}".into(),
                resolved_url: "https://example.com/file.bin".into(),
                entity_tag: "\"etag\"".into(),
                last_modified: "".into(),
                file_name: "file.bin".into(),
                file_path: "/tmp/file.bin".into(),
                status: "Pending".into(),
                downloaded_size: 0,
                total_size: Some(42),
                created_at: None,
                updated_at: None,
            })
            .await
            .unwrap();
        repository
            .save_worker(&DBDownloadWorker {
                id: 91,
                task_id: 9,
                index: 0,
                source_id: Some("primary".into()),
                piece_start: Some(0),
                piece_end: Some(0),
                block_start: Some(0),
                block_end: Some(0),
                start: 0,
                end: 41,
                downloaded: 21,
                status: "Running".into(),
                updated_at: None,
            })
            .await
            .unwrap();

        let reopened = JsonFileRepository::new(&path).await.unwrap();
        let tasks = reopened.load_tasks().await.unwrap();
        let workers = reopened.load_workers(9).await.unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].file_name, "file.bin");
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].downloaded, 21);
    }
}
