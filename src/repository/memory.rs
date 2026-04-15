use super::contract::Repository;
use crate::Error;
use crate::repository::models::{
    DBDownloadChecksum, DBDownloadPiece, DBDownloadTask, DBDownloadWorker,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct MemoryRepository {
    tasks: Arc<RwLock<HashMap<u32, DBDownloadTask>>>,
    workers: Arc<RwLock<HashMap<(u32, u32), DBDownloadWorker>>>,
    pieces: Arc<RwLock<HashMap<(u32, u32), DBDownloadPiece>>>,
    checksums: Arc<RwLock<HashMap<(u32, String), DBDownloadChecksum>>>,
}

impl MemoryRepository {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            workers: Arc::new(RwLock::new(HashMap::new())),
            pieces: Arc::new(RwLock::new(HashMap::new())),
            checksums: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl Repository for MemoryRepository {
    // ---------------- Task ----------------
    async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, Error> {
        let mut tasks: Vec<_> = self.tasks.read().await.values().cloned().collect();
        tasks.sort_by_key(|task| task.id);
        Ok(tasks)
    }

    async fn load_task(&self, task_id: u32) -> Result<Option<DBDownloadTask>, Error> {
        Ok(self.tasks.read().await.get(&task_id).cloned())
    }

    async fn save_task(&self, task: &DBDownloadTask) -> Result<(), Error> {
        self.tasks.write().await.insert(task.id, task.clone());
        Ok(())
    }

    async fn delete_task(&self, task_id: u32) -> Result<(), Error> {
        self.tasks.write().await.remove(&task_id);

        // 删除关联 workers
        let mut workers = self.workers.write().await;
        workers.retain(|_, w| w.task_id != task_id);

        let mut pieces = self.pieces.write().await;
        pieces.retain(|_, piece| piece.task_id != task_id);

        // 删除关联 checksums
        let mut checksums = self.checksums.write().await;
        checksums.retain(|_, c| c.task_id != task_id);

        Ok(())
    }

    // ---------------- Worker ----------------
    async fn load_workers(&self, task_id: u32) -> Result<Vec<DBDownloadWorker>, Error> {
        let mut workers: Vec<_> = self
            .workers
            .read()
            .await
            .values()
            .filter(|w| w.task_id == task_id)
            .cloned()
            .collect();
        workers.sort_by_key(|worker| worker.index);
        Ok(workers)
    }

    async fn load_worker(&self, worker_id: u32) -> Result<Option<DBDownloadWorker>, Error> {
        Ok(self
            .workers
            .read()
            .await
            .values()
            .find(|worker| worker.id == worker_id)
            .cloned())
    }

    async fn save_worker(&self, worker: &DBDownloadWorker) -> Result<(), Error> {
        let mut stored = worker.clone();
        stored.id = worker_storage_id(worker.task_id, worker.index);
        self.workers
            .write()
            .await
            .insert((worker.task_id, worker.index), stored);
        Ok(())
    }

    async fn delete_workers(&self, task_id: u32) -> Result<(), Error> {
        let mut workers = self.workers.write().await;
        workers.retain(|_, w| w.task_id != task_id);
        Ok(())
    }

    async fn load_pieces(&self, task_id: u32) -> Result<Vec<DBDownloadPiece>, Error> {
        let mut pieces: Vec<_> = self
            .pieces
            .read()
            .await
            .values()
            .filter(|piece| piece.task_id == task_id)
            .cloned()
            .collect();
        pieces.sort_by_key(|piece| piece.piece_index);
        Ok(pieces)
    }

    async fn save_pieces(&self, task_id: u32, pieces: &[DBDownloadPiece]) -> Result<(), Error> {
        let mut storage = self.pieces.write().await;
        storage.retain(|(stored_task_id, _), _| *stored_task_id != task_id);
        for piece in pieces {
            storage.insert((task_id, piece.piece_index), piece.clone());
        }
        Ok(())
    }

    async fn delete_pieces(&self, task_id: u32) -> Result<(), Error> {
        let mut pieces = self.pieces.write().await;
        pieces.retain(|_, piece| piece.task_id != task_id);
        Ok(())
    }

    // ---------------- Checksum ----------------
    async fn load_checksums(&self, task_id: u32) -> Result<Vec<DBDownloadChecksum>, Error> {
        let mut checksums: Vec<_> = self
            .checksums
            .read()
            .await
            .values()
            .filter(|c| c.task_id == task_id)
            .cloned()
            .collect();
        checksums.sort_by(|left, right| left.algorithm.cmp(&right.algorithm));
        Ok(checksums)
    }

    async fn load_checksum(&self, checksum_id: u32) -> Result<Option<DBDownloadChecksum>, Error> {
        Ok(self
            .checksums
            .read()
            .await
            .values()
            .find(|checksum| checksum.id == checksum_id)
            .cloned())
    }

    async fn save_checksum(&self, checksum: &DBDownloadChecksum) -> Result<(), Error> {
        let mut stored = checksum.clone();
        stored.id = checksum_storage_id(checksum.task_id, &checksum.algorithm);
        self.checksums
            .write()
            .await
            .insert((checksum.task_id, checksum.algorithm.clone()), stored);
        Ok(())
    }

    async fn delete_checksums(&self, task_id: u32) -> Result<(), Error> {
        let mut checksums = self.checksums.write().await;
        checksums.retain(|_, c| c.task_id != task_id);
        Ok(())
    }
}

fn worker_storage_id(task_id: u32, index: u32) -> u32 {
    ((task_id & 0xFFFF) << 16) | (index & 0xFFFF)
}

fn checksum_storage_id(task_id: u32, algorithm: &str) -> u32 {
    let algorithm_code = match algorithm {
        "MD5" => 1,
        "SHA1" => 2,
        "SHA256" => 3,
        "NONE" => 4,
        _ => 15,
    };
    task_id.saturating_mul(16).saturating_add(algorithm_code)
}

#[cfg(test)]
mod tests {
    use super::MemoryRepository;
    use crate::repository::contract::Repository;
    use crate::repository::models::{DBDownloadChecksum, DBDownloadWorker};

    #[tokio::test]
    async fn keeps_workers_isolated_by_task_and_index() {
        let repository = MemoryRepository::new();

        repository
            .save_worker(&DBDownloadWorker {
                id: 0,
                task_id: 1,
                index: 0,
                start: 0,
                end: 9,
                downloaded: 4,
                status: "Paused".into(),
                updated_at: None,
            })
            .await
            .unwrap();
        repository
            .save_worker(&DBDownloadWorker {
                id: 0,
                task_id: 2,
                index: 0,
                start: 10,
                end: 19,
                downloaded: 7,
                status: "Running".into(),
                updated_at: None,
            })
            .await
            .unwrap();

        let task_one_workers = repository.load_workers(1).await.unwrap();
        let task_two_workers = repository.load_workers(2).await.unwrap();

        assert_eq!(task_one_workers.len(), 1);
        assert_eq!(task_two_workers.len(), 1);
        assert_eq!(task_one_workers[0].task_id, 1);
        assert_eq!(task_one_workers[0].downloaded, 4);
        assert_eq!(task_two_workers[0].task_id, 2);
        assert_eq!(task_two_workers[0].downloaded, 7);
    }

    #[tokio::test]
    async fn keeps_checksums_isolated_by_task_and_algorithm() {
        let repository = MemoryRepository::new();

        repository
            .save_checksum(&DBDownloadChecksum {
                id: 0,
                task_id: 1,
                algorithm: "SHA256".into(),
                value: "aaa".into(),
                verified: true,
                verified_at: None,
            })
            .await
            .unwrap();
        repository
            .save_checksum(&DBDownloadChecksum {
                id: 0,
                task_id: 2,
                algorithm: "SHA256".into(),
                value: "bbb".into(),
                verified: false,
                verified_at: None,
            })
            .await
            .unwrap();

        let task_one_checksums = repository.load_checksums(1).await.unwrap();
        let task_two_checksums = repository.load_checksums(2).await.unwrap();

        assert_eq!(task_one_checksums.len(), 1);
        assert_eq!(task_two_checksums.len(), 1);
        assert_eq!(task_one_checksums[0].value, "aaa");
        assert_eq!(task_two_checksums[0].value, "bbb");
    }
}
