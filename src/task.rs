use crate::checksum::DownloadChecksum;
use crate::chunk::plan_download_chunks;
use crate::config::DownloadConfig;
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::job_finalize::finalize_download;
use crate::job_prepare::{PreparationOutcome, prepare_download};
use crate::manager::DownloadManager;
use crate::persistence::DownloadPersistenceManager;
use crate::stats::DownloadStats;
use crate::status::DownloadStatus;
use crate::worker::DownloadWorker;
use chrono::{DateTime, Utc};
use futures::future::join_all;
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::{Mutex, OnceCell, OwnedSemaphorePermit, RwLock, broadcast};

pub struct DownloadTask {
    pub id: u32,
    pub url: String,
    pub status: Mutex<DownloadStatus>,
    pub file_name: OnceCell<String>,
    pub file_path: Arc<OnceCell<PathBuf>>,
    pub checksums: Mutex<Vec<DownloadChecksum>>,
    pub config: Arc<DownloadConfig>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Mutex<Option<DateTime<Utc>>>,
    pub persistence: Option<Arc<DownloadPersistenceManager>>,
    pub workers: RwLock<Vec<Arc<DownloadWorker>>>,

    pub total_size: AtomicU64,
    pub downloaded_size: AtomicU64,

    pub worker_event_tx: broadcast::Sender<DownloadEvent>,
    pub manager: Weak<DownloadManager>,
    pub stats: Arc<DownloadStats>,
    pub client: Arc<reqwest::Client>,
    pub permit: Mutex<Option<OwnedSemaphorePermit>>,
}

pub type DownloadJob = DownloadTask;

#[derive(Serialize, Deserialize)]
pub struct DownloadTaskSnapshot {
    pub id: u32,
    pub url: String,
    pub file_name: Option<String>,
    pub file_path: Option<PathBuf>,
    pub status: String,
    pub downloaded_size: u64,
    pub total_size: u64,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub checksums: Vec<DownloadChecksum>,
}

pub type DownloadJobSnapshot = DownloadTaskSnapshot;

impl DownloadTask {
    pub fn new(
        id: u32,
        url: String,
        file_name: Option<String>,
        file_path: Option<String>,
        status: Option<DownloadStatus>,
        downloaded_size: Option<u64>,
        total_size: Option<u64>,
        checksums: Vec<DownloadChecksum>,
        client: Arc<reqwest::Client>,
        config: Arc<DownloadConfig>,
        persistence: Option<Arc<DownloadPersistenceManager>>,
        manager: Weak<DownloadManager>,
        created_at: Option<DateTime<Utc>>,
        updated_at: Option<DateTime<Utc>>,
    ) -> Result<Arc<Self>, DownloadError> {
        let (worker_event_tx, _) = broadcast::channel(100);

        // 初始化 file_name OnceCell，如果传入 Some，则立即 set
        let file_name_cell = OnceCell::new();
        if let Some(name) = file_name {
            let _ = file_name_cell.set(name);
        }

        // 初始化 file_path OnceCell，如果传入 Some，则立即 set
        let file_path_cell = Arc::new(OnceCell::new());
        if let Some(path) = file_path {
            let _ = file_path_cell.set(PathBuf::from(path));
        }

        // 初始化 status Mutex
        let initial_status = status.unwrap_or(DownloadStatus::Pending);

        let now = Utc::now();

        Ok(Arc::new(Self {
            id,
            url,
            file_name: file_name_cell,
            file_path: file_path_cell,
            checksums: Mutex::new(checksums),
            status: Mutex::new(initial_status),
            downloaded_size: AtomicU64::new(downloaded_size.unwrap_or(0)),
            config,
            total_size: AtomicU64::new(total_size.unwrap_or(0)),
            created_at: Some(created_at.unwrap_or(now)),
            updated_at: Mutex::new(Some(updated_at.unwrap_or(now))),
            persistence,
            workers: RwLock::new(vec![]),
            worker_event_tx,
            manager,
            stats: Arc::new(DownloadStats::new()),
            client,
            permit: Mutex::new(None),
        }))
    }

    pub async fn snapshot(&self) -> DownloadTaskSnapshot {
        let status_guard = self.status.lock().await;
        let status_str = match &*status_guard {
            DownloadStatus::Failed(err) => format!("Failed: {}", err),
            _ => status_guard.to_string(),
        };

        let updated_at_guard = self.updated_at.lock().await;
        DownloadTaskSnapshot {
            id: self.id,
            url: self.url.clone(),
            file_name: self.file_name.get().cloned(),
            file_path: self.file_path.get().cloned(),
            status: status_str,
            downloaded_size: self.downloaded_size.load(Ordering::Relaxed),
            total_size: self.total_size.load(Ordering::Relaxed),
            created_at: self.created_at.clone(),
            updated_at: updated_at_guard.clone(),
            checksums: self.checksums.lock().await.clone(),
        }
    }

    pub async fn init(self: &Arc<Self>) -> Result<(), DownloadError> {
        debug!("[Task {}] Initializing task: {}", self.id, self.url);

        //Pending
        self.persist_task().await?;
        self.persist_task_checksums().await?;

        let task_clone = Arc::clone(self);
        let stats_clone = Arc::clone(&self.stats);

        let mut rx = self.worker_event_tx.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let task_clone = Arc::clone(&task_clone);

                        match event {
                            DownloadEvent::Progress {
                                id,
                                downloaded: _downloaded,
                                total: _,
                                ..
                            } => {
                                // 保存单个 worker 状态和进度
                                let task_clone_for_persist = Arc::clone(&task_clone);
                                tokio::spawn(async move {
                                    let _ = task_clone_for_persist.persist_task_worker(id).await;
                                });

                                let workers = task_clone.workers.read().await;
                                let total_downloaded: u64 = workers
                                    .iter()
                                    .map(|w| w.downloaded_size.load(Ordering::Relaxed))
                                    .sum();

                                task_clone
                                    .downloaded_size
                                    .store(total_downloaded, Ordering::Relaxed);

                                let total_size = task_clone.total_size.load(Ordering::Relaxed);

                                if let Some(manager) = task_clone.manager.upgrade() {
                                    let _ = manager.task_event_tx.send(DownloadEvent::Progress {
                                        id: task_clone.id,
                                        downloaded: total_downloaded,
                                        total: total_size,
                                    });
                                }
                            }
                            DownloadEvent::Complete(worker_id) => {
                                debug!(
                                    "[Task {} Worker {}] Worker completed its download chunk",
                                    task_clone.id, worker_id
                                );

                                // 保存单个 worker 状态和进度
                                let task_clone_for_persist = Arc::clone(&task_clone);
                                tokio::spawn(async move {
                                    let _ =
                                        task_clone_for_persist.persist_task_worker(worker_id).await;
                                });

                                let workers = task_clone.workers.read().await;
                                let mut all_done = true;
                                for w in workers.iter() {
                                    let status = w.status.lock().await;
                                    if !matches!(*status, DownloadStatus::Completed) {
                                        all_done = false;
                                        debug!(
                                            "[Task {}] Worker Not completed -> id: {}, status: {:?}",
                                            task_clone.id, w.id, *status
                                        );
                                        break;
                                    }
                                }

                                if all_done {
                                    debug!(
                                        "[Task {}] All workers finished, starting checksum verification",
                                        task_clone.id
                                    );

                                    stats_clone.snapshot().await;
                                    if let Err(err) = finalize_download(&task_clone).await {
                                        debug!(
                                            "[Task {}] Finalization failed: {:?}",
                                            task_clone.id, err
                                        );
                                        if let Some(manager) = task_clone.manager.upgrade() {
                                            let _ = manager
                                                .task_event_tx
                                                .send(DownloadEvent::Error(task_clone.id, err));
                                        }
                                    }
                                }
                            }
                            DownloadEvent::Error(worker_id, e) => {
                                debug!(
                                    "[Task {}] Task encountered an error: {:?}",
                                    task_clone.id, e
                                );

                                // 保存单个 worker 状态和进度
                                let task_clone_for_persist = Arc::clone(&task_clone);
                                tokio::spawn(async move {
                                    let _ =
                                        task_clone_for_persist.persist_task_worker(worker_id).await;
                                });

                                stats_clone.snapshot().await;

                                let workers = task_clone.workers.read().await;
                                for w in workers.iter() {
                                    let _ = w.cancel().await;
                                }

                                let mut status = task_clone.status.lock().await;
                                *status = DownloadStatus::Failed(DownloadError::Other(
                                    "Worker error".into(),
                                ));
                                drop(status);

                                if let Some(manager) = task_clone.manager.upgrade() {
                                    let _ = manager.task_event_tx.send(DownloadEvent::Error(
                                        task_clone.id,
                                        DownloadError::Other("Worker error".into()),
                                    ));
                                }
                            }
                            DownloadEvent::Pause(worker_id) => {
                                debug!("[Task {}] Task was paused", task_clone.id);

                                // 保存单个 worker 状态和进度
                                let task_clone_for_persist = Arc::clone(&task_clone);
                                tokio::spawn(async move {
                                    let _ =
                                        task_clone_for_persist.persist_task_worker(worker_id).await;
                                });
                            }
                            DownloadEvent::Cancel(worker_id) => {
                                debug!("[Task {}] Task was cancelled", task_clone.id);

                                // 保存单个 worker 状态和进度
                                let task_clone_for_persist = Arc::clone(&task_clone);
                                tokio::spawn(async move {
                                    let _ =
                                        task_clone_for_persist.persist_task_worker(worker_id).await;
                                });

                                let workers = task_clone.workers.read().await;
                                let mut all_canceled = true;
                                for w in workers.iter() {
                                    let status = w.status.lock().await;
                                    if !matches!(
                                        *status,
                                        DownloadStatus::Canceled | DownloadStatus::Completed
                                    ) {
                                        all_canceled = false;
                                        break;
                                    }
                                }

                                if all_canceled {
                                    let mut status = task_clone.status.lock().await;
                                    *status = DownloadStatus::Canceled;
                                    drop(status);

                                    if let Some(manager) = task_clone.manager.upgrade() {
                                        let _ = manager
                                            .task_event_tx
                                            .send(DownloadEvent::Cancel(task_clone.id));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "[Task {}] Worker event consumer lagged and skipped {} events",
                            task_clone.id, skipped
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        if let Some(manager) = self.manager.upgrade() {
            let _ = manager.task_event_tx.send(DownloadEvent::Pending(self.id));
        }

        Ok(())
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), DownloadError> {
        {
            let status = self.status.lock().await;
            match &*status {
                DownloadStatus::Pending => {
                    debug!(
                        "[Task {}] Task is in Pending state, will attempt to start/restart the task",
                        self.id
                    );
                }
                DownloadStatus::Preparing | DownloadStatus::Running => {
                    debug!(
                        "[Task {}] Task already active: {:?}, skipping start",
                        self.id, *status
                    );
                    return Err(DownloadError::Other(format!(
                        "Task is {:?}, cannot start again",
                        *status
                    )));
                }
                DownloadStatus::Paused => {
                    debug!("[Task {}] Resuming paused task", self.id);
                    drop(status); // 释放锁
                    return self.resume().await; // 如果有 resume() 实现，可直接复用
                }
                DownloadStatus::Completed => {
                    debug!("[Task {}] Task already completed, skipping start", self.id);
                    return Err(DownloadError::Other("Task already completed".into()));
                }
                DownloadStatus::Canceled => {
                    warn!(
                        "[Task {}] Task was previously canceled — restarting as new download",
                        self.id
                    );
                    self.reset_task().await?;
                }

                DownloadStatus::Failed(_) => {
                    warn!(
                        "[Task {}] Task failed previously — resetting and restarting download",
                        self.id
                    );
                    self.reset_task().await?;
                }
                DownloadStatus::Deleted => {
                    debug!("[Task {}] Task has been deleted, cannot start", self.id);
                    return Err(DownloadError::Other("Task deleted".into()));
                }
            }
        }
        //------------------------------------------------------------------------------------------
        {
            let mut status = self.status.lock().await;
            *status = DownloadStatus::Preparing;
            debug!("[Task {}] Preparing download", self.id);
        }

        if let Some(manager) = self.manager.upgrade() {
            let _ = manager
                .task_event_tx
                .send(DownloadEvent::Preparing(self.id));
        }
        //Preparing
        self.persist_task().await?;
        let file_path = match prepare_download(self).await? {
            PreparationOutcome::Ready(prepared) => prepared.file_path,
            PreparationOutcome::Finished => return Ok(()),
        };

        //-----------------------------------------------------------------------
        {
            let mut status = self.status.lock().await;
            *status = DownloadStatus::Running;
            debug!("[Task {}] Starting download", self.id);
        }

        if let Some(manager) = self.manager.upgrade() {
            let _ = manager.task_event_tx.send(DownloadEvent::Start(self.id));
        }

        //Running
        self.persist_task().await?;

        //------------------------------worker-------------------------------------------------
        let mut workers_vec = self.workers.read().await.clone();
        if workers_vec.is_empty() {
            // 如果没有 worker，则创建新的
            let now = Utc::now();
            let chunks = plan_download_chunks(
                self.total_size.load(Ordering::Relaxed),
                self.config.worker_threads,
            );

            for chunk in chunks {
                let worker = Arc::new(DownloadWorker::new(
                    chunk.index,
                    Arc::clone(&self.config),
                    Arc::downgrade(self),
                    Arc::clone(&self.client),
                    self.url.clone(),
                    chunk.start,
                    chunk.end,
                    Some(0),
                    Arc::clone(&file_path),
                    Some(DownloadStatus::Pending),
                    Arc::clone(&self.stats),
                    Some(now),
                ));

                // 持久化 worker
                if let Some(persistence) = self.persistence.as_ref() {
                    // debug!("[Task {}] Persisting worker state", self.id);
                    let worker_clone = Arc::clone(&worker);
                    if let Err(e) = persistence.save_worker(&worker_clone).await {
                        debug!(
                            "[Task {}] Failed to persist worker: {:?}",
                            worker_clone.id, e
                        );
                    }
                }

                workers_vec.push(worker);
            }

            // 写入 self.workers
            let mut workers_lock = self.workers.write().await;
            *workers_lock = workers_vec.clone();
        }

        //---------------------------------------------------------------------------------------
        let task = Arc::clone(&self);
        tokio::spawn(async move {
            let mut tasks = vec![];
            for worker in workers_vec {
                let worker_clone = Arc::clone(&worker);
                tasks.push(tokio::spawn(async move { worker_clone.start().await }));
            }

            let results = join_all(tasks).await;

            for r in results {
                match r {
                    Ok(inner) => match inner {
                        Ok(()) => {}
                        Err(e) => {
                            let mut status = task.status.lock().await;
                            *status =
                                DownloadStatus::Failed(DownloadError::Other(format!("{:?}", e)));
                            debug!("[Task {}] Worker failed: {:?}", task.id, e);

                            if let Some(manager) = task.manager.upgrade() {
                                let _ = manager.task_event_tx.send(DownloadEvent::Error(
                                    task.id,
                                    DownloadError::Other(format!("{:?}", e)),
                                ));
                            }
                        }
                    },
                    Err(e) => {
                        let mut status = task.status.lock().await;
                        *status = DownloadStatus::Failed(DownloadError::Other(format!("{:?}", e)));
                        debug!("[Task {}] Worker panicked: {:?}", task.id, e);

                        if let Some(manager) = task.manager.upgrade() {
                            let _ = manager.task_event_tx.send(DownloadEvent::Error(
                                task.id,
                                DownloadError::Other(format!("{:?}", e)),
                            ));
                        }
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn pause(self: &Arc<Self>) -> Result<(), DownloadError> {
        let mut status = self.status.lock().await;
        match *status {
            DownloadStatus::Running | DownloadStatus::Preparing => {
                *status = DownloadStatus::Paused;
                debug!("[Task {}] Paused", self.id);
            }
            DownloadStatus::Paused => {
                debug!("[Task {}] Task is already paused", self.id);
                return Ok(());
            }
            DownloadStatus::Pending
            | DownloadStatus::Completed
            | DownloadStatus::Canceled
            | DownloadStatus::Deleted
            | DownloadStatus::Failed(_) => {
                return Err(DownloadError::Other(format!(
                    "Cannot pause task in state: {:?}",
                    *status
                )));
            }
        }
        drop(status);

        let workers = self.workers.read().await;
        for worker in workers.iter() {
            let _ = worker.pause().await;
        }

        //pause
        self.persist_task().await?;

        if let Some(manager) = self.manager.upgrade() {
            let _ = manager.task_event_tx.send(DownloadEvent::Pause(self.id));
        }

        Ok(())
    }

    pub async fn resume(self: &Arc<Self>) -> Result<(), DownloadError> {
        let mut status = self.status.lock().await;
        match *status {
            DownloadStatus::Pending | DownloadStatus::Running => {
                drop(status);
                // 将递归调用包裹成 boxed future
                return Box::pin(self.start()).await;
            }
            DownloadStatus::Paused => {
                *status = match self.total_size.load(Ordering::Relaxed) {
                    0 => {
                        debug!(
                            "[Task {}] Resuming paused task in Preparing/Pending phase",
                            self.id
                        );
                        DownloadStatus::Preparing
                    }
                    _ => {
                        debug!("[Task {}] Resuming paused task", self.id);
                        DownloadStatus::Running
                    }
                };
            }
            DownloadStatus::Completed
            | DownloadStatus::Preparing
            | DownloadStatus::Canceled
            | DownloadStatus::Deleted
            | DownloadStatus::Failed(_) => {
                return Err(DownloadError::Other(format!(
                    "Cannot resume task in state: {:?}",
                    *status
                )));
            }
        }
        drop(status);

        //resume
        self.persist_task().await?;

        let status = self.status.lock().await;
        if matches!(*status, DownloadStatus::Running) {
            let workers = self.workers.read().await;
            for worker in workers.iter() {
                let _ = worker.resume().await;
            }
            if let Some(manager) = self.manager.upgrade() {
                let _ = manager.task_event_tx.send(DownloadEvent::Start(self.id));
            }
        }

        Ok(())
    }

    pub async fn cancel(self: &Arc<Self>) -> Result<(), DownloadError> {
        let mut status = self.status.lock().await;
        match *status {
            DownloadStatus::Pending
            | DownloadStatus::Preparing
            | DownloadStatus::Running
            | DownloadStatus::Paused => {
                *status = DownloadStatus::Canceled;
                debug!("[Task {}] Canceled", self.id);
            }
            DownloadStatus::Completed
            | DownloadStatus::Canceled
            | DownloadStatus::Deleted
            | DownloadStatus::Failed(_) => {
                return Err(DownloadError::Other(format!(
                    "Cannot cancel task in state: {:?}",
                    *status
                )));
            }
        }
        drop(status);

        //task
        self.persist_task().await?;

        //workers
        let workers = self.workers.write().await;
        for worker in workers.iter() {
            let _ = worker.cancel().await;
        }

        if let Some(manager) = self.manager.upgrade() {
            let _ = manager.task_event_tx.send(DownloadEvent::Cancel(self.id));
        }

        Ok(())
    }

    pub async fn delete(self: &Arc<Self>) -> Result<(), DownloadError> {
        //workers
        let mut workers = self.workers.write().await;
        for worker in workers.iter() {
            let _ = worker.delete().await;
        }
        workers.clear();

        //file
        self.delete_task_file().await?;

        self.purge_task_workers().await?;
        //task
        self.purge_task_checksums().await?;
        //task
        self.purge_task().await?;

        //
        if let Some(manager) = self.manager.upgrade() {
            let _ = manager.task_event_tx.send(DownloadEvent::Delete(self.id));
        }
        Ok(())
    }

    async fn reset_task(self: &Arc<Self>) -> Result<(), DownloadError> {
        //workers
        self.clear_task_workers().await?;
        self.purge_task_workers().await?;

        //progress
        self.downloaded_size.store(0, Ordering::Relaxed);
        self.total_size.store(0, Ordering::Relaxed);

        //file
        self.delete_task_file().await?;

        Ok(())
    }

    async fn delete_task_file(self: &Arc<Self>) -> Result<(), DownloadError> {
        if let Some(file_path) = self.file_path.get() {
            if file_path.exists() {
                match tokio::fs::remove_file(file_path).await {
                    Ok(_) => debug!("[Task {}] Removed file: {:?}", self.id, file_path),
                    Err(e) => debug!(
                        "[Task {}] Failed to remove file {:?}: {}",
                        self.id, file_path, e
                    ),
                }
            } else {
                debug!("[Task {}] File not found: {:?}", self.id, file_path);
            }
        }
        Ok(())
    }

    async fn clear_task_workers(self: &Arc<Self>) -> Result<(), DownloadError> {
        let mut workers = self.workers.write().await;
        workers.clear();

        Ok(())
    }

    pub async fn persist_task(self: &Arc<Self>) -> Result<(), DownloadError> {
        {
            let mut updated_at_guard = self.updated_at.lock().await;
            *updated_at_guard = Some(Utc::now());
        }

        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Persisting task", self.id);
            if let Err(e) = persistence.save_task(self).await {
                debug!("[Task {}] Failed to persist task: {:?}", self.id, e);
            }
        }
        Ok(())
    }

    async fn purge_task(self: &Arc<Self>) -> Result<(), DownloadError> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Deleting task", self.id);
            if let Err(e) = persistence.delete_task(self.id).await {
                debug!("[Task {}] Failed to delete task: {:?}", self.id, e);
            }
            if let Err(e) = persistence.delete_workers(self.id).await {
                debug!("[Task {}] Failed to delete task worker: {:?}", self.id, e);
            }
        }
        Ok(())
    }

    async fn purge_task_workers(self: &Arc<Self>) -> Result<(), DownloadError> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Deleting task workers", self.id);
            if let Err(e) = persistence.delete_workers(self.id).await {
                debug!("[Task {}] Failed to delete task worker: {:?}", self.id, e);
            }
        }
        Ok(())
    }

    async fn purge_task_checksums(self: &Arc<Self>) -> Result<(), DownloadError> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Deleting task checksums", self.id);
            if let Err(e) = persistence.delete_checksums(self.id).await {
                debug!(
                    "[Task {}] Failed to delete task checksums: {:?}",
                    self.id, e
                );
            }
        }
        Ok(())
    }

    pub async fn persist_task_checksums(self: &Arc<Self>) -> Result<(), DownloadError> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Persisting task checksums", self.id);
            let checksums = self.checksums.lock().await;
            if let Err(e) = persistence.save_checksums(&checksums, self.id).await {
                debug!(
                    "[Task {}] Failed to persist task checksums: {:?}",
                    self.id, e
                );
            }
        }
        Ok(())
    }

    pub async fn persist_task_worker(
        self: &Arc<Self>,
        worker_id: u32,
    ) -> Result<(), DownloadError> {
        if let Some(persistence) = self.persistence.as_ref() {
            let worker_opt = {
                let workers = self.workers.read().await;
                workers.iter().find(|w| w.id == worker_id).cloned()
            };

            if let Some(worker) = worker_opt {
                // debug!("[Task {}] Persisting worker {}", self.id, worker_id);
                {
                    let mut updated_at_guard = worker.updated_at.lock().await;
                    *updated_at_guard = Some(Utc::now());
                }
                if let Err(e) = persistence.save_worker(&worker).await {
                    debug!(
                        "[Task {}] Failed to persist worker {}: {:?}",
                        self.id, worker_id, e
                    );
                }
            } else {
                debug!(
                    "[Task {}] Worker {} not found, cannot persist",
                    self.id, worker_id
                );
            }
        }
        Ok(())
    }

    //-----------------------------------------------------------------------------------------------
    pub(crate) fn resolve_or_init_file_name(&self) -> String {
        if let Some(name) = self.file_name.get() {
            return name.clone();
        }

        let file_name = if let Ok(url) = url::Url::parse(&self.url) {
            url.path_segments()
                .and_then(|segments| segments.last())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("download_{}.tmp", self.id))
        } else {
            format!("download_{}.tmp", self.id)
        };

        let _ = self.file_name.set(file_name.clone());
        file_name
    }

    /// 获取或初始化文件路径
    pub(crate) fn get_or_init_file_path(
        &self,
        download_dir: &Path,
    ) -> Result<Arc<PathBuf>, DownloadError> {
        // 1. 如果已经设置过，就直接返回
        if let Some(existing_path) = self.file_path.get() {
            let file_path = Arc::new(existing_path.clone());
            debug!(
                "[Task {}] Using existing file path: {:?}",
                self.id, file_path
            );
            return Ok(file_path);
        }

        // 2. 构造文件名（从 file_name 获取，没设置则用 task_id）
        let file_name = self.resolve_or_init_file_name();

        // 3. 构造文件路径
        let path = download_dir.join(&file_name);

        // 4. 只在第一次设置时生效
        let _ = self.file_path.set(path);

        // 5. 获取 OnceCell 中的 PathBuf
        let file_path_ref = self
            .file_path
            .get()
            .ok_or_else(|| DownloadError::Other("file_path not set".into()))?;

        // 6. 转成 Arc<PathBuf> 共享使用
        let file_path = Arc::new(file_path_ref.clone());

        debug!("[Task {}] Initialized file path: {:?}", self.id, file_path);

        Ok(file_path)
    }
}

impl fmt::Debug for DownloadTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadTask")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("status", &self.status)
            .field("file_name", &self.file_name)
            .field("file_path", &self.file_path)
            .field("checksums", &self.checksums)
            .field("downloaded_size", &self.downloaded_size)
            .field("config", &self.config)
            .field("workers", &self.workers)
            .field("total_size", &self.total_size)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            //.field("listener", &self.listener)
            .finish()
    }
}
