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
use log::{LevelFilter, debug, error, warn};
use std::collections::VecDeque;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::fs;
use tokio::sync::{Mutex, OnceCell, broadcast, mpsc};

/**
 *
 */
pub struct DownloadManager {
    pub config: Arc<DownloadConfig>,
    pub tasks: Arc<DashMap<u32, Arc<DownloadTask>>>,
    pub persistence: OnceCell<Arc<DownloadPersistenceManager>>,
    pub status_tx: Option<mpsc::Sender<DownloadStatus>>,
    pub command_rx: Mutex<Option<mpsc::Receiver<crate::cli::Command>>>,
    pub running_count: Arc<AtomicUsize>,
    pub pending_queue: Arc<Mutex<VecDeque<u32>>>,
    pub task_event_tx: broadcast::Sender<DownloadEvent>,
}

impl DownloadManager {
    pub fn new(
        config: DownloadConfig,
        status_tx: Option<mpsc::Sender<DownloadStatus>>,
        command_rx: Option<mpsc::Receiver<crate::cli::Command>>,
    ) -> Result<Arc<Self>, DownloadError> {
        let (task_event_tx, _) = broadcast::channel(100);

        let manager = Arc::new(Self {
            config: Arc::new(config),
            tasks: Arc::new(DashMap::new()),
            persistence: OnceCell::new(),
            status_tx,
            command_rx: Mutex::new(command_rx),
            running_count: Arc::new(AtomicUsize::new(0)),
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
                        manager_clone
                            .persist_task(task_id)
                            .await
                            .expect(&format!("Failed to persist task: {}", task_id));

                        manager_clone
                            .spawn_next_task()
                            .await
                            .expect("Failed to spawn the next download task after an error event");
                    }
                    DownloadEvent::Cancel(task_id) => {
                        manager_clone
                            .persist_task(task_id)
                            .await
                            .expect(&format!("Failed to persist task: {}", task_id));

                        manager_clone
                            .spawn_next_task()
                            .await
                            .expect("Failed to spawn the next download task after an error event");
                    }
                    DownloadEvent::Error(task_id, error) => {
                        manager_clone
                            .persist_task(task_id)
                            .await
                            .expect(&format!("Failed to persist task: {}", task_id));

                        if let Some(tx) = &manager_clone.status_tx {
                            let _ = tx.send(DownloadStatus::Failed(error)).await;
                        }

                        manager_clone
                            .spawn_next_task()
                            .await
                            .expect("Failed to spawn the next download task after an error event");
                    }
                    DownloadEvent::Preparing(task_id) => {
                        manager_clone
                            .persist_task(task_id)
                            .await
                            .expect(&format!("Failed to persist task: {}", task_id));

                        if let Some(tx) = &manager_clone.status_tx {
                            let _ = tx.send(DownloadStatus::Preparing).await;
                        }
                    }
                    DownloadEvent::Pause(task_id) => {
                        manager_clone
                            .persist_task(task_id)
                            .await
                            .expect(&format!("Failed to persist task: {}", task_id));

                        if let Some(tx) = &manager_clone.status_tx {
                            let _ = tx.send(DownloadStatus::Paused).await;
                        }
                    }
                    DownloadEvent::Progress {
                        id,
                        downloaded: _downloaded,
                        total: _total,
                    } => {
                        manager_clone
                            .persist_task(id)
                            .await
                            .expect(&format!("Failed to persist task: {}", id));

                        if let Some(tx) = &manager_clone.status_tx {
                            let _ = tx.send(DownloadStatus::Running).await;
                        }
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }

    async fn spawn_next_task(self: &Arc<Self>) -> Result<(), DownloadError> {
        self.running_count.fetch_sub(1, Ordering::Relaxed);

        let next_task_id_opt = {
            let mut queue = self.pending_queue.lock().await;
            queue.pop_front()
        };
        if let Some(next_id) = next_task_id_opt {
            let manager_clone = Arc::clone(&self);
            tokio::spawn(async move {
                let _ = manager_clone.start_task(next_id).await;
            });
        }
        Ok(())
    }

    /// 从数据库加载所有任务并恢复到内存
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

            // 5. 转换 task -> DownloadTaskRequest
            let task_request = DownloadTaskRequest {
                id: Some(db_task.id),
                url: db_task.url.clone(),
                file_name: Some(db_task.file_name.clone()),
                file_path: Some(db_task.file_path.clone()),
                status: Some(
                    DownloadStatus::from_str(&db_task.status).unwrap_or(DownloadStatus::Pending),
                ),
                downloaded_size: Some(db_task.downloaded_size),
                total_size: db_task.total_size,
                checksums: Some(checksums),
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
            return Ok(*existing.key());
        }

        //
        let task_id = task_request.id.unwrap_or_else(|| {
            let mut new_id = self.tasks.len() as u32 + 1;
            while self.tasks.contains_key(&new_id) {
                new_id += 1;
            }
            new_id
        });

        let persistence = self
            .persistence
            .get()
            .cloned()
            .expect("PersistenceManager not initialized");

        let task = DownloadTask::new(
            task_id,
            task_request.url,
            task_request.file_name.clone(), // 对应 Option<String>
            task_request.file_path.clone(), // 对应 Option<String>
            task_request.status,            // 对应 Option<DownloadStatus>
            task_request.downloaded_size,   // 对应 Option<u64>
            task_request.total_size,        // 对应 Option<u64>
            task_request.checksums.clone().unwrap_or_default(), // 对应 Vec<DownloadChecksum>
            self.config.clone(),
            Some(persistence),
            Arc::downgrade(self),
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
        let max_concurrent_tasks = self.config.max_concurrent_downloads;

        if self.running_count.load(Ordering::Relaxed) >= max_concurrent_tasks {
            let mut queue = self.pending_queue.lock().await;
            if !queue.contains(&task_id) {
                queue.push_back(task_id);
            }
            return Ok(task_id);
        }

        if let Some(task_ref) = self.tasks.get(&task_id) {
            let task = Arc::clone(task_ref.value());
            let tx_status = self.status_tx.clone();
            let running_count = self.running_count.clone();
            running_count.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                let result = task.start().await;
                match result {
                    Ok(_) => {
                        if let Some(tx) = tx_status {
                            let _ = tx.send(DownloadStatus::Running).await;
                        }
                    }
                    Err(e) => {
                        if let Some(tx) = tx_status {
                            let _ = tx
                                .send(DownloadStatus::Failed(DownloadError::Other(e.to_string())))
                                .await;
                        }
                    }
                }
            });
            Ok(task_id)
        } else {
            Err(DownloadError::TaskNotFound(task_id))
        }
    }

    pub async fn pause_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        if let Some(task_ref) = self.tasks.get(&task_id) {
            let task = Arc::clone(task_ref.value());
            task.pause().await?;
            if let Some(tx) = &self.status_tx {
                let _ = tx.send(DownloadStatus::Paused).await;
            }
            Ok(task_id)
        } else {
            Err(DownloadError::TaskNotFound(task_id))
        }
    }

    pub async fn resume_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        if let Some(task_ref) = self.tasks.get(&task_id) {
            let task = Arc::clone(task_ref.value());
            task.resume().await?;
            if let Some(tx) = &self.status_tx {
                let _ = tx.send(DownloadStatus::Running).await;
            }
            Ok(task_id)
        } else {
            Err(DownloadError::TaskNotFound(task_id))
        }
    }

    pub async fn cancel_task(self: &Arc<Self>, task_id: u32) -> Result<u32, DownloadError> {
        if let Some(task_ref) = self.tasks.get(&task_id) {
            let task = Arc::clone(task_ref.value());
            task.cancel().await?;
            if let Some(tx) = &self.status_tx {
                let _ = tx.send(DownloadStatus::Canceled).await;
            }
            Ok(task_id)
        } else {
            Err(DownloadError::TaskNotFound(task_id))
        }
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

        // 3.执行保存操作
        match persistence.save_task(&task).await {
            Ok(_) => {
                debug!("[Manager {}] ✅ Task state persisted successfully", task_id);
                Ok(task_id)
            }
            Err(e) => {
                error!(
                    "[Manager {}] ❌ Failed to persist task state: {:?}",
                    task_id, e
                );
                Err(e)
            }
        }
    }

    pub async fn start_all(self: &Arc<Self>) -> Result<(), DownloadError> {
        let futures: Vec<_> = self
            .tasks
            .iter()
            .map(|entry| {
                let task = Arc::clone(entry.value());
                tokio::spawn(async move { task.start().await })
            })
            .collect();

        for f in futures {
            let _ = f.await;
        }
        Ok(())
    }

    pub async fn pause_all(self: &Arc<Self>) -> Result<(), DownloadError> {
        let futures: Vec<_> = self
            .tasks
            .iter()
            .map(|entry| {
                let task = Arc::clone(entry.value());
                tokio::spawn(async move { task.pause().await })
            })
            .collect();

        for f in futures {
            let _ = f.await;
        }
        Ok(())
    }

    pub async fn resume_all(self: &Arc<Self>) -> Result<(), DownloadError> {
        let futures: Vec<_> = self
            .tasks
            .iter()
            .map(|entry| {
                let task = Arc::clone(entry.value());
                tokio::spawn(async move { task.resume().await })
            })
            .collect();

        for f in futures {
            let _ = f.await;
        }
        Ok(())
    }

    pub async fn cancel_all(self: &Arc<Self>) -> Result<(), DownloadError> {
        for entry in self.tasks.iter() {
            let task = Arc::clone(entry.value());
            task.cancel().await?;
        }
        if let Some(tx) = &self.status_tx {
            let _ = tx.send(DownloadStatus::Canceled).await;
        }
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

    pub async fn run(self: &Arc<Self>) -> Result<(), DownloadError> {
        let mut rx_opt = self.command_rx.lock().await;
        if rx_opt.is_none() {
            return Ok(());
        }
        let mut rx = rx_opt.take().unwrap();
        drop(rx_opt);
        while let Some(cmd) = rx.recv().await {
            match cmd {
                crate::cli::Command::Pause => self.pause_all().await?,
                crate::cli::Command::Resume => self.resume_all().await?,
                crate::cli::Command::Cancel => self.cancel_all().await?,
                crate::cli::Command::SetRateLimit(limit) => {
                    println!("Setting rate limit to {} KB/s", limit);
                }
            }
        }
        Ok(())
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<DownloadEvent> {
        self.task_event_tx.subscribe()
    }
}
