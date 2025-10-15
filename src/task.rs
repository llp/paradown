use crate::checksum::DownloadChecksum;
use crate::config::{DownloadConfig, FileConflictStrategy};
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::manager::DownloadManager;
use crate::persistence::DownloadPersistenceManager;
use crate::stats::DownloadStats;
use crate::status::DownloadStatus;
use crate::worker::DownloadWorker;
use chrono::{DateTime, Utc};
use futures::future::join_all;
use log::{debug, info};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::fs;
use tokio::sync::{Mutex, OnceCell, RwLock, broadcast};
use url::Url;

pub struct DownloadTask {
    pub id: u32,
    pub url: String,
    pub status: Mutex<DownloadStatus>,
    pub file_name: OnceCell<String>,
    pub file_path: Arc<OnceCell<PathBuf>>,
    pub checksums: Vec<DownloadChecksum>,
    pub config: Arc<DownloadConfig>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub persistence: Option<Arc<DownloadPersistenceManager>>,
    pub workers: RwLock<Vec<Arc<DownloadWorker>>>,

    pub total_size: AtomicU64,
    pub progress: AtomicU64,

    pub worker_event_tx: broadcast::Sender<DownloadEvent>,
    pub manager: Weak<DownloadManager>,
    pub stats: Arc<DownloadStats>,
    pub client: Arc<reqwest::Client>,
}

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
        config: Arc<DownloadConfig>,
        persistence: Option<Arc<DownloadPersistenceManager>>,
        manager: Weak<DownloadManager>,
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

        let client = Arc::new(
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(300))
                .pool_max_idle_per_host(50)
                .pool_idle_timeout(Duration::from_secs(60))
                .gzip(true)
                .build()
                .map_err(|e| DownloadError::Other(format!("Failed to build HTTP client: {}", e)))?,
        );

        Ok(Arc::new(Self {
            id,
            url,
            file_name: file_name_cell,
            file_path: file_path_cell,
            checksums,
            status: Mutex::new(initial_status),
            progress: AtomicU64::new(downloaded_size.unwrap_or(0)),
            config,
            total_size: AtomicU64::new(total_size.unwrap_or(0)),
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
            persistence,
            workers: RwLock::new(vec![]),
            worker_event_tx,
            manager,
            stats: Arc::new(DownloadStats::new()),
            client,
        }))
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
            while let Ok(event) = rx.recv().await {
                let task_clone = Arc::clone(&task_clone);

                match event {
                    DownloadEvent::Progress {
                        id,
                        downloaded,
                        total,
                        ..
                    } => {
                        // 保存单个 worker 状态和进度
                        let task_clone_for_persist = Arc::clone(&task_clone);
                        tokio::spawn(async move {
                            task_clone_for_persist.persist_task_worker(id).await;
                        });

                        let workers = task_clone.workers.read().await;
                        let total_downloaded: u64 = workers
                            .iter()
                            .map(|w| w.downloaded.load(Ordering::Relaxed))
                            .sum();

                        task_clone
                            .progress
                            .store(total_downloaded, Ordering::Relaxed);

                        let total_size = task_clone.total_size.load(Ordering::Relaxed);
                        debug!(
                            "[Task {}] Progress: {}/{} ({:.2}%)",
                            task_clone.id,
                            total_downloaded,
                            total_size,
                            total_downloaded as f64 / total_size as f64 * 100.0
                        );

                        if let Some(manager) = task_clone.manager.upgrade() {
                            let _ = manager.task_event_tx.send(DownloadEvent::Progress {
                                id: task_clone.id,
                                downloaded: total_downloaded,
                                total,
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
                            task_clone_for_persist.persist_task_worker(worker_id).await;
                        });

                        let workers = task_clone.workers.read().await;
                        let mut all_done = true;
                        for w in workers.iter() {
                            let status = w.status.lock().await;
                            if !matches!(*status, DownloadStatus::Completed) {
                                all_done = false;
                                break;
                            }
                        }

                        if all_done {
                            debug!(
                                "[Task {}] All workers finished, starting checksum verification",
                                task_clone.id
                            );

                            stats_clone.snapshot().await;

                            let checksum_ok = if let Some(path) = task_clone.file_path.get() {
                                let mut ok = true;
                                for checksum in &task_clone.checksums {
                                    match checksum.verify(path.as_ref()) {
                                        Ok(true) => {
                                            debug!(
                                                "[Task {}] Checksum {:?} passed for file {:?}",
                                                task_clone.id, checksum.algorithm, path
                                            );
                                        }
                                        Ok(false) => {
                                            debug!(
                                                "[Task {}] Checksum {:?} FAILED for file {:?}",
                                                task_clone.id, checksum.algorithm, path
                                            );
                                            ok = false;
                                            break;
                                        }
                                        Err(e) => {
                                            debug!(
                                                "[Task {}] Checksum {:?} ERROR for file {:?}: {:?}",
                                                task_clone.id, checksum.algorithm, path, e
                                            );
                                            ok = false;
                                            break;
                                        }
                                    }
                                }
                                ok
                            } else {
                                debug!(
                                    "[Task {}] No file path set for task, cannot verify checksum",
                                    task_clone.id
                                );
                                false
                            };

                            let mut status = task_clone.status.lock().await;
                            *status = if checksum_ok {
                                debug!(
                                    "[Task {}] All workers finished and checksum passed",
                                    task_clone.id
                                );
                                debug!("[Task {}] Download task completed", task_clone.id);
                                DownloadStatus::Completed
                            } else {
                                debug!("[Task {}] Checksum verification failed", task_clone.id);
                                DownloadStatus::Failed(DownloadError::Other(
                                    "Checksum failed".into(),
                                ))
                            };

                            if let Some(manager) = task_clone.manager.upgrade() {
                                let _ = manager
                                    .task_event_tx
                                    .send(DownloadEvent::Complete(task_clone.id));
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
                            task_clone_for_persist.persist_task_worker(worker_id).await;
                        });

                        stats_clone.snapshot().await;

                        let workers = task_clone.workers.read().await;
                        for w in workers.iter() {
                            let _ = w.cancel().await;
                        }

                        let mut status = task_clone.status.lock().await;
                        *status =
                            DownloadStatus::Failed(DownloadError::Other("Worker error".into()));

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
                            task_clone_for_persist.persist_task_worker(worker_id).await;
                        });
                    }
                    DownloadEvent::Cancel(worker_id) => {
                        debug!("[Task {}] Task was cancelled", task_clone.id);

                        // 保存单个 worker 状态和进度
                        let task_clone_for_persist = Arc::clone(&task_clone);
                        tokio::spawn(async move {
                            task_clone_for_persist.persist_task_worker(worker_id).await;
                        });

                        let workers = task_clone.workers.read().await;
                        let mut all_canceled = true;
                        for w in workers.iter() {
                            let status = w.status.lock().await;
                            if !matches!(*status, DownloadStatus::Completed) {
                                all_canceled = false;
                                break;
                            }
                        }

                        if all_canceled {
                            let mut status = task_clone.status.lock().await;
                            *status = DownloadStatus::Canceled;

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
        });

        Ok(())
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), DownloadError> {
        {
            let status = self.status.lock().await;
            if matches!(*status, DownloadStatus::Completed) {
                debug!("[Task {}] Task already completed, skipping start", self.id);
                return Ok(());
            }
        }
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

        //-----------------------------------------------------------------------
        let download_dir = &self.config.download_dir;
        if !Path::new(download_dir).exists() {
            fs::create_dir_all(download_dir).await.map_err(|e| {
                DownloadError::Io(format!("Failed to create download directory: {}", e))
            })?;
            debug!(
                "[Task {}] Created download directory: {}",
                self.id,
                download_dir.display()
            );
        }

        //---------------------------------file_name---------------------------------------------
        let file_name = if let Some(name) = self.file_name.get() {
            name.clone()
        } else if let Ok(url) = Url::parse(&self.url) {
            url.path_segments()
                .and_then(|segments| segments.last())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("download_{}.tmp", self.id))
        } else {
            format!("download_{}.tmp", self.id)
        };

        let _ = self.file_name.set(file_name.clone());

        //---------------------------------file_path---------------------------------------------
        let file_path = self.get_or_init_file_path(download_dir)?;
        debug!("[Task {}] Download file path: {:?}", self.id, file_path);

        //-----------------------------------total_size-------------------------------------------
        let total_size = if self.total_size.load(Ordering::Relaxed) == 0 {
            let resp = self.client.head(&self.url).send().await?;
            if !resp.status().is_success() {
                return Err(format!("Failed to fetch file info: {}", resp.status()).into());
            }
            let size = resp
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .ok_or("No Content-Length")?
                .to_str()?
                .parse::<u64>()?;
            self.total_size.store(size, Ordering::Relaxed);
            size
        } else {
            self.total_size.load(Ordering::Relaxed)
        };
        debug!("[Task {}] Total file size: {} bytes", self.id, total_size);

        //-----------------------------------------------------------------------
        if file_path.as_ref().exists() {
            let metadata = fs::metadata(file_path.as_ref())
                .await
                .map_err(|e| DownloadError::Io(format!("Failed to read file metadata: {}", e)))?;
            let current_size = metadata.len();

            let checksum_ok = self
                .checksums
                .iter()
                .all(|c| c.verify(file_path.as_ref()).unwrap_or(false));

            match self.config.file_conflict_strategy {
                FileConflictStrategy::Overwrite => {
                    fs::remove_file(file_path.as_ref()).await.map_err(|e| {
                        DownloadError::Io(format!("Failed to remove existing file: {}", e))
                    })?;
                    debug!("[Task {}] Overwriting existing file", self.id);
                }
                FileConflictStrategy::SkipIfValid => {
                    if checksum_ok {
                        {
                            let mut status = self.status.lock().await;
                            *status = DownloadStatus::Completed;
                        }
                        info!(
                            "[Task {}] File exists and valid, skipping download",
                            self.id
                        );
                        if let Some(manager) = self.manager.upgrade() {
                            let _ = manager.task_event_tx.send(DownloadEvent::Complete(self.id));
                        }
                        return Ok(());
                    } else {
                        debug!(
                            "[Task {}] File exists but invalid, will redownload",
                            self.id
                        );
                        fs::remove_file(file_path.as_ref()).await?;
                    }
                }
                FileConflictStrategy::Resume => {
                    if current_size < self.total_size.load(Ordering::Relaxed) {
                        debug!(
                            "[Task {}] Resuming download from byte {}",
                            self.id, current_size
                        );
                    } else if checksum_ok {
                        {
                            let mut status = self.status.lock().await;
                            *status = DownloadStatus::Completed;
                        }
                        info!(
                            "[Task {}] File exists and valid, skipping download",
                            self.id
                        );
                        if let Some(manager) = self.manager.upgrade() {
                            let _ = manager.task_event_tx.send(DownloadEvent::Complete(self.id));
                        }
                        return Ok(());
                    } else {
                        debug!(
                            "[Task {}] File exists but invalid, will redownload",
                            self.id
                        );
                        fs::remove_file(file_path.as_ref()).await?;
                    }
                }
            }
        }
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
            let worker_count = self.config.worker_threads;
            let total_size = self.total_size.load(Ordering::Relaxed);
            let chunk_size = total_size / worker_count as u64;

            for i in 0..worker_count {
                let start = i as u64 * chunk_size;
                let end = if i == worker_count - 1 {
                    total_size - 1
                } else {
                    (i as u64 + 1) * chunk_size - 1
                };

                let worker = Arc::new(DownloadWorker::new(
                    i as u32,
                    Arc::clone(&self.config),
                    Arc::downgrade(self),
                    Arc::clone(&self.client),
                    self.url.clone(),
                    start,
                    end,
                    Some(0),
                    Arc::clone(&file_path),
                    Some(DownloadStatus::Pending),
                    Arc::clone(&self.stats),
                ));

                // 持久化 worker
                if let Some(persistence) = self.persistence.as_ref() {
                    debug!("[Task {}] Persisting worker state", self.id);
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
                        let mut status = self.status.lock().await;
                        *status = DownloadStatus::Failed(DownloadError::Other(format!("{:?}", e)));
                        debug!("[Task {}] Worker failed: {:?}", self.id, e);

                        if let Some(manager) = self.manager.upgrade() {
                            let _ = manager.task_event_tx.send(DownloadEvent::Error(
                                self.id,
                                DownloadError::Other(format!("{:?}", e)),
                            ));
                        }
                        return Err(format!("Worker failed: {:?}", e).into());
                    }
                },
                Err(e) => {
                    let mut status = self.status.lock().await;
                    *status = DownloadStatus::Failed(DownloadError::Other(format!("{:?}", e)));
                    debug!("[Task {}] Worker panicked: {:?}", self.id, e);

                    if let Some(manager) = self.manager.upgrade() {
                        let _ = manager.task_event_tx.send(DownloadEvent::Error(
                            self.id,
                            DownloadError::Other(format!("{:?}", e)),
                        ));
                    }

                    return Err(format!("Worker panicked: {:?}", e).into());
                }
            }
        }

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
            DownloadStatus::Preparing => {
                debug!(
                    "[Task {}] Task is still preparing, will start when ready",
                    self.id
                );
                return Ok(());
            }
            DownloadStatus::Pending => {
                debug!("[Task {}] Task pending, starting now", self.id);
                *status = DownloadStatus::Preparing;
            }
            DownloadStatus::Running => {
                debug!("[Task {}] Task is already running", self.id);
                return Ok(());
            }
            DownloadStatus::Completed | DownloadStatus::Canceled | DownloadStatus::Failed(_) => {
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
            DownloadStatus::Completed | DownloadStatus::Canceled | DownloadStatus::Failed(_) => {
                return Err(DownloadError::Other(format!(
                    "Cannot cancel task in state: {:?}",
                    *status
                )));
            }
        }
        drop(status);

        //cancel
        self.persist_task().await?;

        let workers = self.workers.read().await;
        for worker in workers.iter() {
            let _ = worker.cancel().await;
        }

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

        if let Some(manager) = self.manager.upgrade() {
            let _ = manager.task_event_tx.send(DownloadEvent::Cancel(self.id));
        }

        Ok(())
    }

    pub async fn persist_task(self: &Arc<Self>) -> Result<(), DownloadError> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Persisting task", self.id);
            if let Err(e) = persistence.save_task(self).await {
                debug!("[Task {}] Failed to persist task: {:?}", self.id, e);
            }
        }
        Ok(())
    }

    pub async fn persist_task_checksums(self: &Arc<Self>) -> Result<(), DownloadError> {
        if let Some(persistence) = self.persistence.as_ref() {
            debug!("[Task {}] Persisting task checksums", self.id);
            if let Err(e) = persistence.save_checksums(&self.checksums, self.id).await {
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
                debug!("[Task {}] Persisting worker {}", self.id, worker_id);
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
    /// 获取或初始化文件路径
    fn get_or_init_file_path(&self, download_dir: &Path) -> Result<Arc<PathBuf>, DownloadError> {
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
        let file_name = self
            .file_name
            .get()
            .cloned()
            .unwrap_or_else(|| format!("download_{}.tmp", self.id));

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
            .field("progress", &self.progress)
            .field("config", &self.config)
            .field("workers", &self.workers)
            .field("total_size", &self.total_size)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            //.field("listener", &self.listener)
            .finish()
    }
}
