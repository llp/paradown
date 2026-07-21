use crate::error::Error;
use crate::repository::models::{
    DBDownloadBlock, DBDownloadChecksum, DBDownloadPiece, DBDownloadTask, DBDownloadWorker,
};
use async_trait::async_trait;

/// 仓储接口 trait。
///
/// `pub trait Repository: Send + Sync` 的含义：
/// - `pub trait Repository` 定义一个公开 trait。
/// - `: Send + Sync` 要求所有实现该 trait 的类型也必须满足线程安全相关 trait bound。
///
/// trait 中的方法只有签名而没有方法体时，实现者必须自己实现。
/// 带默认方法体的 `save_bundle` 则可以被实现者直接复用，也可以被覆盖。
///
/// `#[async_trait]` 让 trait 内可以书写 `async fn`。
/// 这属于宏生成代码的场景，系统解释见 `docs/rust/generics-and-traits.md`。
#[async_trait]
pub trait Repository: Send + Sync {
    async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, Error>;
    async fn load_task(&self, task_id: u32) -> Result<Option<DBDownloadTask>, Error>;
    async fn save_task(&self, task: &DBDownloadTask) -> Result<(), Error>;
    async fn delete_task(&self, task_id: u32) -> Result<(), Error>;
    /// 默认方法实现。
    ///
    /// `&[DBDownloadWorker]` 是 slice 引用，表示借用一段连续数据，不拥有它。
    /// `for worker in workers` 遍历 slice 时，`worker` 的类型是 `&DBDownloadWorker`。
    ///
    /// 每个 `.await?` 先等待异步操作完成，再在错误时提前返回。
    async fn save_bundle(
        &self,
        task: &DBDownloadTask,
        workers: &[DBDownloadWorker],
        pieces: &[DBDownloadPiece],
        blocks: &[DBDownloadBlock],
        checksums: &[DBDownloadChecksum],
    ) -> Result<(), Error> {
        self.save_task(task).await?;
        self.delete_workers(task.id).await?;
        for worker in workers {
            self.save_worker(worker).await?;
        }
        self.save_pieces(task.id, pieces).await?;
        self.save_blocks(task.id, blocks).await?;
        self.delete_checksums(task.id).await?;
        for checksum in checksums {
            self.save_checksum(checksum).await?;
        }
        Ok(())
    }

    async fn load_workers(&self, task_id: u32) -> Result<Vec<DBDownloadWorker>, Error>;
    async fn load_worker(&self, worker_id: u32) -> Result<Option<DBDownloadWorker>, Error>;
    async fn save_worker(&self, worker: &DBDownloadWorker) -> Result<(), Error>;
    async fn delete_workers(&self, task_id: u32) -> Result<(), Error>;

    async fn load_pieces(&self, task_id: u32) -> Result<Vec<DBDownloadPiece>, Error>;
    async fn save_pieces(&self, task_id: u32, pieces: &[DBDownloadPiece]) -> Result<(), Error>;
    async fn delete_pieces(&self, task_id: u32) -> Result<(), Error>;

    async fn load_blocks(&self, task_id: u32) -> Result<Vec<DBDownloadBlock>, Error>;
    async fn save_blocks(&self, task_id: u32, blocks: &[DBDownloadBlock]) -> Result<(), Error>;
    async fn delete_blocks(&self, task_id: u32) -> Result<(), Error>;

    async fn load_checksums(&self, task_id: u32) -> Result<Vec<DBDownloadChecksum>, Error>;
    async fn load_checksum(&self, checksum_id: u32) -> Result<Option<DBDownloadChecksum>, Error>;
    async fn save_checksum(&self, checksum: &DBDownloadChecksum) -> Result<(), Error>;
    async fn delete_checksums(&self, task_id: u32) -> Result<(), Error>;
}
