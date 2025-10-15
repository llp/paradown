use crate::error::DownloadError;
use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
use async_trait::async_trait;

#[async_trait]
pub trait DownloadRepository: Send + Sync {
    //
    async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, DownloadError>;
    async fn load_task(&self, task_id: u32) -> Result<Option<DBDownloadTask>, DownloadError>;
    async fn save_task(&self, task: &DBDownloadTask) -> Result<(), DownloadError>;
    async fn delete_task(&self, task_id: u32) -> Result<(), DownloadError>;

    //
    async fn load_workers(&self, task_id: u32) -> Result<Vec<DBDownloadWorker>, DownloadError>;
    async fn load_worker(&self, worker_id: u32) -> Result<Option<DBDownloadWorker>, DownloadError>;
    async fn save_worker(&self, worker: &DBDownloadWorker) -> Result<(), DownloadError>;
    async fn delete_workers(&self, task_id: u32) -> Result<(), DownloadError>;

    //
    async fn load_checksums(&self, task_id: u32) -> Result<Vec<DBDownloadChecksum>, DownloadError>;
    async fn load_checksum(
        &self,
        checksum_id: u32,
    ) -> Result<Option<DBDownloadChecksum>, DownloadError>;
    async fn save_checksum(&self, checksum: &DBDownloadChecksum) -> Result<(), DownloadError>;
    async fn delete_checksums(&self, task_id: u32) -> Result<(), DownloadError>;
}
