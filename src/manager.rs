use crate::config::DownloadConfig;
use crate::coordinator_events::spawn_task_event_loop;
use crate::coordinator_queue::{
    acquire_task_permit_or_queue, clear_pending_queue, spawn_next_task,
};
use crate::coordinator_registry::{add_task_with_workers, restore_tasks};
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::persistence::DownloadPersistenceManager;
use crate::request::DownloadTaskRequest;
use crate::runtime::build_http_client;
use crate::task::DownloadTask;
use dashmap::DashMap;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use log::{debug, error, info};
use std::collections::VecDeque;
use std::path::Path;
use std::pin::Pin;
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
    pub http_client: Arc<reqwest::Client>,

    pub(crate) semaphore: Arc<Semaphore>,
}

pub type DownloadCoordinator = DownloadManager;

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
        let http_client = Arc::new(build_http_client(&config)?);

        let manager = Arc::new(Self {
            config: Arc::new(config),
            tasks: Arc::new(DashMap::new()),
            persistence: OnceCell::new(),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            pending_queue: Arc::new(Mutex::new(VecDeque::new())),
            task_event_tx,
            http_client,
        });

        Ok(manager)
    }

    pub async fn init(self: &Arc<Self>) -> Result<(), DownloadError> {
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

        restore_tasks(self).await?;

        spawn_task_event_loop(Arc::clone(self));

        Ok(())
    }

    pub async fn add_task(
        self: &Arc<Self>,
        task_request: DownloadTaskRequest,
    ) -> Result<u32, DownloadError> {
        add_task_with_workers(self, task_request, None).await
    }

    pub async fn start_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        self.run_task_transition(
            task_id,
            PendingAction::Start,
            |task| Box::pin(async move { task.start().await }),
            "Starting",
        )
        .await
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
        self.run_task_transition(
            task_id,
            PendingAction::Resume,
            |task| Box::pin(async move { task.resume().await }),
            "Resuming",
        )
        .await
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

    pub(crate) async fn persist_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
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

        let checksums = task.checksums.lock().await;
        persistence.save_checksums(&checksums, task_id).await?;

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

        clear_pending_queue(self).await?;

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
                    if let Err(e) = spawn_next_task(&manager_clone).await {
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

        clear_pending_queue(self).await?;

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

        clear_pending_queue(self).await?;

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

    async fn run_task_transition<'a, F>(
        self: &'a Arc<Self>,
        task_id: u32,
        action: PendingAction,
        operation: F,
        action_label: &str,
    ) -> Result<u32, DownloadError>
    where
        F: FnOnce(
            Arc<DownloadTask>,
        )
            -> Pin<Box<dyn std::future::Future<Output = Result<(), DownloadError>> + Send + 'a>>,
    {
        let Some(permit) = acquire_task_permit_or_queue(self, task_id, action).await? else {
            return Ok(task_id);
        };

        let task_ref = self
            .tasks
            .get(&task_id)
            .ok_or(DownloadError::TaskNotFound(task_id))?;
        let task = Arc::clone(task_ref.value());

        debug!(
            "[Manager] {} task {} (permits left after acquire = {})",
            action_label,
            task_id,
            self.semaphore.available_permits()
        );

        let res = operation(Arc::clone(&task)).await;
        match res {
            Ok(()) => {
                debug!("[Manager] Task {} {} successfully", task_id, action_label);
                let mut guard = task.permit.lock().await;
                *guard = Some(permit);
                Ok(task_id)
            }
            Err(e) => {
                error!(
                    "[Manager] Task {} {} failed: {:?}",
                    task_id, action_label, e
                );
                drop(permit);
                let mut guard = task.permit.lock().await;
                *guard = None;

                if let Err(spawn_err) = spawn_next_task(self).await {
                    error!(
                        "Failed to spawn next task after {} task failed({}): {:?}",
                        action_label.to_lowercase(),
                        task_id,
                        spawn_err
                    );
                }
                Err(e)
            }
        }
    }
}
