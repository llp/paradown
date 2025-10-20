use crate::config::DownloadConfig;
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::persistence::DownloadPersistenceManager;
use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
use crate::request::{DownloadTaskRequest, DownloadWorkerRequest};
use crate::status::DownloadStatus;
use crate::task::DownloadTask;
use crate::worker::DownloadWorker;
use dashmap::DashMap;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use log::{LevelFilter, debug, error, info, warn};
use std::collections::VecDeque;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::{Mutex, OnceCell, Semaphore, broadcast};

/**
 *
 */
pub struct DownloadManager {
    pub config: Arc<DownloadConfig>,
    pub tasks: Arc<DashMap<u32, Arc<DownloadTask>>>,
    pub persistence: OnceCell<Arc<DownloadPersistenceManager>>,
    pub pending_queue: Arc<Mutex<VecDeque<(u32, PendingAction)>>>,
    pub task_event_tx: broadcast::Sender<DownloadEvent>,

    semaphore: Arc<Semaphore>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingAction {
    Start,
    Resume,
    Retry,
}

impl DownloadManager {
    pub fn new(config: DownloadConfig) -> Result<Arc<Self>, DownloadError> {
        let (task_event_tx, _) = broadcast::channel(100);
        let max_concurrent = config.max_concurrent_downloads;

        let manager = Arc::new(Self {
            config: Arc::new(config),
            tasks: Arc::new(DashMap::new()),
            persistence: OnceCell::new(),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            pending_queue: Arc::new(Mutex::new(VecDeque::new())),
            task_event_tx,
        });

        Ok(manager)
    }

    pub async fn init(self: &Arc<Self>) -> Result<(), DownloadError> {
        let log_level = if self.config.debug {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        };

        env_logger::Builder::from_default_env()
            .filter_level(log_level)
            .filter_module("sqlx::query", LevelFilter::Info) // 只让 sqlx 输出 info 级别以上
            .init();

        let persistence = Arc::new(DownloadPersistenceManager::new(self.config.clone()).await?);
        self.persistence
            .set(persistence)
            .map_err(|_| DownloadError::ConfigError("Persistence already initialized".into()))?;

        //
        let download_dir = &self.config.download_dir;
        if !Path::new(download_dir).exists() {
            fs::create_dir_all(download_dir)
                .await
                .map_err(|e| DownloadError::Io(format!("创建下载目录失败: {}", e)))?;
        }

        //
        self.load_tasks().await?;

        let manager_clone = Arc::clone(self);
        let mut rx = self.task_event_tx.subscribe();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                match event {
                    DownloadEvent::Complete(task_id) => {
                        if let Err(e) = manager_clone.persist_task(task_id).await {
                            error!("Failed to persist task {}: {:?}", task_id, e);
                        }
                        //----------------------------------------------------------
                        //
                        if let Err(e) = manager_clone.remove_from_queue(task_id).await {
                            error!("Failed remove_from_queue on Complete({}): {:?}", task_id, e);
                        }
                        //
                        manager_clone.release_task_permit(task_id).await;
                        //
                        if let Err(e) = manager_clone.spawn_next_task().await {
                            error!(
                                "Failed to spawn next task after Complete({}): {:?}",
                                task_id, e
                            );
                        }
                    }
                    DownloadEvent::Cancel(task_id) => {
                        if let Err(e) = manager_clone.persist_task(task_id).await {
                            error!("Failed to persist task {}: {:?}", task_id, e);
                        }

                        //----------------------------------------------------------
                        if let Err(e) = manager_clone.remove_from_queue(task_id).await {
                            error!("Failed remove_from_queue on Complete({}): {:?}", task_id, e);
                        }
                        //
                        manager_clone.release_task_permit(task_id).await;
                        //
                        if let Err(e) = manager_clone.spawn_next_task().await {
                            error!(
                                "Failed to spawn next task after Complete({}): {:?}",
                                task_id, e
                            );
                        }
                    }
                    DownloadEvent::Error(task_id, _err) => {
                        if let Err(e) = manager_clone.persist_task(task_id).await {
                            error!("Failed to persist task {}: {:?}", task_id, e);
                        }

                        //----------------------------------------------------------
                        if let Err(e) = manager_clone.remove_from_queue(task_id).await {
                            error!("Failed remove_from_queue on Complete({}): {:?}", task_id, e);
                        }
                        //
                        manager_clone.release_task_permit(task_id).await;
                        //
                        if let Err(e) = manager_clone.spawn_next_task().await {
                            error!(
                                "Failed to spawn next task after Complete({}): {:?}",
                                task_id, e
                            );
                        }
                    }
                    DownloadEvent::Preparing(task_id) => {
                        if let Err(e) = manager_clone.persist_task(task_id).await {
                            error!("Failed to persist task {}: {:?}", task_id, e);
                        }
                    }
                    DownloadEvent::Pause(task_id) => {
                        if let Err(e) = manager_clone.persist_task(task_id).await {
                            error!("Failed to persist task {}: {:?}", task_id, e);
                        }
                        //----------------------------------------------------------
                        if let Err(e) = manager_clone.remove_from_queue(task_id).await {
                            error!("Failed remove_from_queue on Complete({}): {:?}", task_id, e);
                        }
                        //
                        manager_clone.release_task_permit(task_id).await;
                        //
                        if let Err(e) = manager_clone.spawn_next_task().await {
                            error!(
                                "Failed to spawn next task after Complete({}): {:?}",
                                task_id, e
                            );
                        }
                    }
                    DownloadEvent::Progress { id, .. } => {
                        if let Err(e) = manager_clone.persist_task(id).await {
                            error!("Failed to persist task {}: {:?}", id, e);
                        }
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }

    async fn release_task_permit(&self, task_id: u32) {
        if let Some(task) = self.get_task(task_id) {
            let mut guard = task.permit.lock().await;
            if guard.is_some() {
                *guard = None; // permit drop -> semaphore slot 释放
                debug!("[Task {}] Released semaphore permit", task_id);
            }
        } else {
            warn!("[Manager] Task {} not found when releasing permit", task_id);
        }
        debug!(
            "[Manager] release task {} permit (permits left after release = {})",
            task_id,
            self.semaphore.available_permits()
        );
    }

    /// 将 task_id 从等待队列中移除（如果存在）
    async fn remove_from_queue(&self, task_id: u32) -> Result<(), DownloadError> {
        let mut queue = self.pending_queue.lock().await;
        if let Some(pos) = queue.iter().position(|(id, _action)| *id == task_id) {
            queue.remove(pos);
            debug!(
                "[Manager] Removed task {} from pending queue (new len={})",
                task_id,
                queue.len()
            );
        }
        Ok(())
    }

    async fn clear_task_pending_queue(&self) -> Result<(), DownloadError> {
        let mut queue = self.pending_queue.lock().await;
        queue.clear();
        Ok(())
    }

    /// spawn 下一个排队任务（会跳过启动失败或不存在的任务）
    async fn spawn_next_task(self: &Arc<Self>) -> Result<(), DownloadError> {
        debug!("[Manager] spawn_next_task invoked");

        let next_opt = {
            let mut queue = self.pending_queue.lock().await;
            let queue_len_before = queue.len();
            debug!("[Manager] Queue length before pop: {}", queue_len_before);
            queue.pop_front()
        };

        if let Some((task_id, action)) = next_opt {
            let manager_clone = Arc::clone(self);
            match action {
                PendingAction::Start => {
                    if let Err(e) = manager_clone.start_task(task_id).await {
                        error!("[Manager] Failed to start queued task {}: {:?}", task_id, e);
                    }
                }
                PendingAction::Resume => {
                    if let Err(e) = manager_clone.resume_task(task_id).await {
                        error!(
                            "[Manager] Failed to resume queued task {}: {:?}",
                            task_id, e
                        );
                    }
                }
                PendingAction::Retry => {
                    if let Err(e) = manager_clone.start_task(task_id).await {
                        error!("[Manager] Failed to retry task {}: {:?}", task_id, e);
                    }
                }
            }
        };

        let queue_len_after = {
            let queue = self.pending_queue.lock().await;
            queue.len()
        };
        debug!(
            "[Manager] spawn_next_task completed. Queue length after task = {}",
            queue_len_after
        );
        Ok(())
    }

    /// 从数据库加载并恢复任务（保持你原逻辑，仅保留修正 Running->Paused）
    async fn load_tasks(self: &Arc<Self>) -> Result<(), DownloadError> {
        // 1. 获取持久化组件
        let persistence = self
            .persistence
            .get()
            .ok_or_else(|| DownloadError::ConfigError("Persistence not initialized".into()))?;

        // 2. 从数据库加载所有任务
        let db_tasks: Vec<DBDownloadTask> = persistence.load_tasks().await?;

        for db_task in db_tasks {
            // 3. 读取对应的 workers 和 checksums
            let db_workers: Vec<DBDownloadWorker> = persistence
                .load_workers(db_task.id)
                .await
                .unwrap_or_default();

            let db_checksums: Vec<DBDownloadChecksum> = persistence
                .load_checksums(db_task.id)
                .await
                .unwrap_or_default();

            // 4. 转换 checksums
            let checksums = db_checksums
                .into_iter()
                .map(|c| persistence.db_to_checksum(&c))
                .collect::<Vec<_>>();

            // 5. 修正 Running/Preparing 状态
            let mut restored_status =
                DownloadStatus::from_str(&db_task.status).unwrap_or(DownloadStatus::Pending);
            restored_status = match restored_status {
                DownloadStatus::Running | DownloadStatus::Preparing => DownloadStatus::Paused,
                _ => restored_status,
            };

            // 6. 转换 task -> DownloadTaskRequest
            let task_request = DownloadTaskRequest {
                id: Some(db_task.id),
                url: db_task.url.clone(),
                file_name: Some(db_task.file_name.clone()),
                file_path: Some(db_task.file_path.clone()),
                status: Some(restored_status),
                downloaded_size: Some(db_task.downloaded_size),
                total_size: db_task.total_size,
                checksums: Some(checksums),
                created_at: db_task.created_at,
                updated_at: db_task.updated_at,
            };

            // 6. 转换 workers -> DownloadWorkerRequest
            let workers: Option<Vec<DownloadWorkerRequest>> = if !db_workers.is_empty() {
                Some(
                    db_workers
                        .into_iter()
                        .map(|w| DownloadWorkerRequest {
                            id: Some(w.id),
                            task_id: w.task_id,
                            index: w.index,
                            start: w.start,
                            end: w.end,
                            downloaded: Some(w.downloaded),
                            status: Some(w.status.clone()),
                            updated_at: w.updated_at,
                        })
                        .collect(),
                )
            } else {
                None
            };
            // 7. 调用 add_task_request 恢复任务
            if let Err(e) = self.add_task_with_workers(task_request, workers).await {
                log::error!("Failed to restore task {}: {:?}", db_task.id, e);
            }
        }

        Ok(())
    }

    pub async fn add_task(
        self: &Arc<Self>,
        task_request: DownloadTaskRequest,
    ) -> Result<u32, DownloadError> {
        self.add_task_with_workers(task_request, None).await
    }

    async fn add_task_with_workers(
        self: &Arc<Self>,
        task_request: DownloadTaskRequest,
        workers: Option<Vec<DownloadWorkerRequest>>,
    ) -> Result<u32, DownloadError> {
        //
        if let Some(existing) = self
            .tasks
            .iter()
            .find(|entry| entry.value().url == task_request.url)
        {
            warn!(
                "[DownloadManager] Task with URL '{}' already exists, returning existing task_id {}",
                task_request.url,
                *existing.key()
            );
            return Err(DownloadError::Other(format!(
                "URL '{}' already exists!",
                task_request.url
            )));
        }

        //
        let task_id = task_request.id.unwrap_or_else(|| {
            let mut new_id = self.tasks.len() as u32 + 1;
            while self.tasks.contains_key(&new_id) {
                new_id += 1;
            }
            new_id
        });

        let file_path = task_request
            .file_path
            .clone()
            .filter(|path| !path.trim().is_empty());

        let file_name = task_request
            .file_name
            .clone()
            .filter(|path| !path.trim().is_empty());

        let persistence = self
            .persistence
            .get()
            .cloned()
            .expect("PersistenceManager not initialized");

        let task = DownloadTask::new(
            task_id,
            task_request.url,
            file_name,                                          // 对应 Option<String>
            file_path,                                          // 对应 Option<String>
            task_request.status,                                // 对应 Option<DownloadStatus>
            task_request.downloaded_size,                       // 对应 Option<u64>
            task_request.total_size,                            // 对应 Option<u64>
            task_request.checksums.clone().unwrap_or_default(), // 对应 Vec<DownloadChecksum>
            self.config.clone(),
            Some(persistence),
            Arc::downgrade(self),
            task_request.created_at,
            task_request.updated_at,
        )?;
        if let Some(worker_requests) = workers {
            let mut worker_vec = vec![];
            for w_req in worker_requests.into_iter() {
                let worker = DownloadWorker::new(
                    w_req.index,
                    self.config.clone(),
                    Arc::downgrade(&task),
                    task.client.clone(),
                    task.url.clone(),
                    w_req.start,
                    w_req.end,
                    w_req.downloaded,
                    Arc::new(task_request.file_path.clone().unwrap_or_default().into()),
                    w_req
                        .status
                        .as_ref()
                        .and_then(|s| DownloadStatus::from_str(s).ok()),
                    task.stats.clone(),
                    w_req.updated_at,
                );
                worker_vec.push(Arc::new(worker));
            }
            let mut task_workers = task.workers.write().await;
            *task_workers = worker_vec;
        }

        task.init().await?;
        self.tasks.insert(task_id, task);
        Ok(task_id)
    }

    pub async fn start_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        // 尝试获取 semaphore permit
        let permit = match self.semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                // semaphore 满 -> 加入队列
                let mut queue = self.pending_queue.lock().await;
                if !queue.iter().any(|(id, _)| *id == task_id) {
                    queue.push_back((task_id, PendingAction::Start));
                    debug!(
                        "[Manager] Task {} queued for start (queue len = {}, permits={})",
                        task_id,
                        queue.len(),
                        self.semaphore.available_permits()
                    );
                } else {
                    debug!(
                        "[Manager] Task {} already in queue (skip push). queue len={}",
                        task_id,
                        queue.len()
                    );
                    if let Err(e) = Box::pin(self.spawn_next_task()).await {
                        error!(
                            "Failed to spawn next task after start task failed({}): {:?}",
                            task_id, e
                        );
                    }
                }
                return Ok(task_id);
            }
        };

        let task_ref = self
            .tasks
            .get(&task_id)
            .ok_or(DownloadError::TaskNotFound(task_id))?;
        let task = Arc::clone(task_ref.value());

        debug!(
            "[Manager] Starting task {} (permits left after acquire = {})",
            task_id,
            self.semaphore.available_permits()
        );

        let res = task.start().await;
        match res {
            Ok(()) => {
                debug!("[Manager] Task {} started", task_id);
                let mut guard = task.permit.lock().await;
                *guard = Some(permit);
                Ok(task_id)
            }
            Err(e) => {
                error!("[Manager] Task {} start failed: {:?}", task_id, e);
                drop(permit);
                let mut guard = task.permit.lock().await;
                *guard = None;

                if let Err(e) = Box::pin(self.spawn_next_task()).await {
                    error!(
                        "Failed to spawn next task after start task failed({}): {:?}",
                        task_id, e
                    );
                }
                Err(e)
            }
        }
    }

    pub async fn pause_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        if let Some(task_ref) = self.tasks.get(&task_id) {
            let task = Arc::clone(task_ref.value());
            task.pause().await?;
            Ok(task_id)
        } else {
            Err(DownloadError::TaskNotFound(task_id))
        }
    }

    pub async fn resume_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        // 尝试获取 semaphore permit
        let permit = match self.semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                // semaphore 满 -> 加入队列
                let mut queue = self.pending_queue.lock().await;
                if !queue.iter().any(|(id, _)| *id == task_id) {
                    queue.push_back((task_id, PendingAction::Resume));
                    debug!(
                        "[Manager] Task {} queued for resume (queue len = {}, permits={})",
                        task_id,
                        queue.len(),
                        self.semaphore.available_permits()
                    );
                } else {
                    debug!(
                        "[Manager] Task {} already in queue (skip push). queue len={}",
                        task_id,
                        queue.len()
                    );
                    if let Err(e) = Box::pin(self.spawn_next_task()).await {
                        error!(
                            "Failed to spawn next task after start task failed({}): {:?}",
                            task_id, e
                        );
                    }
                }
                return Ok(task_id);
            }
        };

        let task_ref = self
            .tasks
            .get(&task_id)
            .ok_or(DownloadError::TaskNotFound(task_id))?;
        let task = Arc::clone(task_ref.value());

        debug!(
            "[Manager] Resuming task {} (permits left after acquire = {})",
            task_id,
            self.semaphore.available_permits()
        );

        let res = task.resume().await;
        match res {
            Ok(()) => {
                debug!("[Manager] Task {} resumed", task_id);
                let mut guard = task.permit.lock().await;
                *guard = Some(permit);
                Ok(task_id)
            }
            Err(e) => {
                error!("[Manager] Task {} resume failed: {:?}", task_id, e);
                drop(permit);
                let mut guard = task.permit.lock().await;
                *guard = None;

                if let Err(e) = Box::pin(self.spawn_next_task()).await {
                    error!(
                        "Failed to spawn next task after resume task failed({}): {:?}",
                        task_id, e
                    );
                }
                Err(e)
            }
        }
    }

    pub async fn cancel_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        if let Some(task_ref) = self.tasks.get(&task_id) {
            let task = Arc::clone(task_ref.value());
            task.cancel().await?;
            Ok(task_id)
        } else {
            Err(DownloadError::TaskNotFound(task_id))
        }
    }

    pub async fn delete_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        let task = {
            if let Some(task_ref) = self.tasks.get(&task_id) {
                Arc::clone(task_ref.value())
            } else {
                error!("[Task {}] Not found when trying to delete", task_id);
                return Err(DownloadError::TaskNotFound(task_id));
            }
        };

        if let Err(e) = task.delete().await {
            error!("[Task {}] Failed to delete task: {:?}", task_id, e);
            return Err(e);
        }
        self.tasks.remove(&task_id);
        info!(
            "[Task {}] Deleted successfully and removed from map",
            task_id
        );
        Ok(task_id)
    }

    async fn persist_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        // 1.获取任务
        let Some(task) = self.get_task(task_id) else {
            error!(
                "[Manager] Task {} not found while persisting state",
                task_id
            );
            return Err(DownloadError::Other(format!("Task {} not found", task_id)));
        };

        // 2.获取持久化管理器
        let Some(persistence) = &task.persistence else {
            debug!(
                "[Manager {}] No persistence layer attached, skipping save",
                task_id
            );
            return Ok(task_id);
        };
        //
        persistence.save_checksums(&task.checksums, task_id).await?;

        // 3.执行保存操作
        match persistence.save_task(&task).await {
            Ok(_) => {
                // debug!("[Manager {}] Task state persisted successfully", task_id);
                Ok(task_id)
            }
            Err(e) => {
                error!(
                    "[Manager {}] Failed to persist task state: {:?}",
                    task_id, e
                );
                Err(e)
            }
        }
    }

    pub async fn start_all(self: &Arc<Self>) -> Result<(), DownloadError> {
        if self.tasks.is_empty() {
            return Err(DownloadError::Other("No tasks to start".into()));
        }

        // 使用 FuturesUnordered 并发尝试启动每个任务（但实际启动受 semaphore 控制）
        let mut futures = FuturesUnordered::new();

        for entry in self.tasks.iter() {
            let task_id = *entry.key();
            let manager_clone = Arc::clone(self);
            futures.push(tokio::spawn(async move {
                if let Err(e) = manager_clone.start_task(task_id).await {
                    // 打印错误，同时不阻塞其他任务
                    error!("[Task {}] Failed to start: {:?}", task_id, e);
                }
            }));
        }

        // 等待所有尝试入队/启动的任务完成（不等待下载本身完成）
        while let Some(_) = futures.next().await {}

        Ok(())
    }

    pub async fn pause_all(self: &Arc<Self>) -> Result<(), DownloadError> {
        if self.tasks.is_empty() {
            return Err(DownloadError::Other("No tasks to pause".into()));
        }

        self.clear_task_pending_queue().await?;

        let futures: Vec<_> = self
            .tasks
            .iter()
            .map(|entry| {
                let task = Arc::clone(entry.value());
                tokio::spawn(async move {
                    if let Err(e) = task.pause().await {
                        error!("[Task {}] Failed to pause: {:?}", task.id, e);
                    }
                })
            })
            .collect();

        for f in futures {
            let _ = f.await;
        }
        Ok(())
    }

    pub async fn resume_all(self: &Arc<Self>) -> Result<(), DownloadError> {
        if self.tasks.is_empty() {
            return Err(DownloadError::Other("No tasks to resume".into()));
        }

        let mut futures = FuturesUnordered::new();

        for entry in self.tasks.iter() {
            let task_id = *entry.key();
            let manager_clone = Arc::clone(self);
            futures.push(tokio::spawn(async move {
                if let Err(e) = manager_clone.resume_task(task_id).await {
                    error!("[Task {}] Failed to resume: {:?}", task_id, e);
                    if let Err(e) = manager_clone.spawn_next_task().await {
                        error!("[Manager] Failed to spawn next task after resume failure of task {}: {:?}", task_id, e);
                    }
                }
            }));
        }
        while let Some(_) = futures.next().await {}

        Ok(())
    }

    pub async fn cancel_all(self: &Arc<Self>) -> Result<(), DownloadError> {
        if self.tasks.is_empty() {
            return Err(DownloadError::Other("No tasks to cancel".into()));
        }

        self.clear_task_pending_queue().await?;

        for entry in self.tasks.iter() {
            let task = Arc::clone(entry.value());
            if let Err(e) = task.cancel().await {
                error!("[Task {}] Failed to cancel: {:?}", task.id, e);
            }
        }
        Ok(())
    }

    pub async fn delete_all(self: &Arc<Self>) -> Result<(), DownloadError> {
        if self.tasks.is_empty() {
            return Err(DownloadError::Other("No tasks to delete".into()));
        }

        self.clear_task_pending_queue().await?;

        for entry in self.tasks.iter() {
            let task = Arc::clone(entry.value());
            if let Err(e) = task.delete().await {
                error!("Failed to delete task {}: {:?}", task.id, e);
            }
        }
        self.tasks.clear();
        Ok(())
    }

    pub fn get_task(&self, id: u32) -> Option<Arc<DownloadTask>> {
        self.get_task_by_id(id)
    }

    pub fn get_task_by_id(&self, id: u32) -> Option<Arc<DownloadTask>> {
        self.tasks.get(&id).map(|v| Arc::clone(&v))
    }

    pub fn get_task_by_url(&self, url: &str) -> Option<Arc<DownloadTask>> {
        self.tasks
            .iter()
            .find(|entry| entry.value().url == url)
            .map(|entry| Arc::clone(entry.value()))
    }

    pub fn get_all_tasks(&self) -> Vec<Arc<DownloadTask>> {
        self.tasks
            .iter()
            .map(|entry| Arc::clone(&*entry.value()))
            .collect()
    }

    //----------------------------------------------------------------------------------------------
    pub fn subscribe_events(&self) -> broadcast::Receiver<DownloadEvent> {
        self.task_event_tx.subscribe()
    }
}
