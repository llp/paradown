use crate::checksum::DownloadChecksum;
use crate::config::DownloadConfig;
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::job_prepare::{PreparationOutcome, prepare_download};
use crate::job_state::StartDirective;
use crate::manager::DownloadManager;
use crate::persistence::DownloadPersistenceManager;
use crate::stats::DownloadStats;
use crate::status::DownloadStatus;
use crate::worker::DownloadWorker;
use chrono::{DateTime, Utc};
use log::debug;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    pub range_requests_supported: AtomicBool,
    pub protocol_probe_completed: AtomicBool,

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

        let file_name_cell = OnceCell::new();
        if let Some(name) = file_name {
            let _ = file_name_cell.set(name);
        }

        let file_path_cell = Arc::new(OnceCell::new());
        if let Some(path) = file_path {
            let _ = file_path_cell.set(PathBuf::from(path));
        }

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
            range_requests_supported: AtomicBool::new(false),
            protocol_probe_completed: AtomicBool::new(false),
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

        self.persist_task().await?;
        self.persist_task_checksums().await?;
        self.spawn_worker_event_listener();
        self.emit_manager_event(DownloadEvent::Pending(self.id));

        Ok(())
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), DownloadError> {
        match self.prepare_for_start().await? {
            StartDirective::Continue => {}
            StartDirective::Resume => return self.resume().await,
        }

        self.set_status(DownloadStatus::Preparing).await;
        debug!("[Task {}] Preparing download", self.id);
        self.emit_manager_event(DownloadEvent::Preparing(self.id));
        self.persist_task().await?;

        let file_path = match prepare_download(self).await? {
            PreparationOutcome::Ready(prepared) => prepared.file_path,
            PreparationOutcome::Finished => return Ok(()),
        };

        self.set_status(DownloadStatus::Running).await;
        debug!("[Task {}] Starting download", self.id);
        self.emit_manager_event(DownloadEvent::Start(self.id));
        self.persist_task().await?;

        let workers = self.ensure_workers(&file_path).await?;
        self.spawn_workers(workers);

        Ok(())
    }

    pub(crate) fn emit_manager_event(&self, event: DownloadEvent) {
        if let Some(manager) = self.manager.upgrade() {
            let _ = manager.task_event_tx.send(event);
        }
    }

    pub(crate) fn update_protocol_probe(&self, total_size: u64, supports_range_requests: bool) {
        self.total_size.store(total_size, Ordering::Relaxed);
        self.range_requests_supported
            .store(supports_range_requests, Ordering::Relaxed);
        self.protocol_probe_completed.store(true, Ordering::Relaxed);
    }

    pub(crate) fn supports_range_requests(&self) -> bool {
        self.range_requests_supported.load(Ordering::Relaxed)
    }

    pub(crate) fn protocol_probe_completed(&self) -> bool {
        self.protocol_probe_completed.load(Ordering::Relaxed)
    }

    pub(crate) fn resolve_or_init_file_name(&self) -> String {
        if let Some(name) = self.file_name.get() {
            return name.clone();
        }

        let file_name = if let Ok(url) = url::Url::parse(&self.url) {
            url.path_segments()
                .and_then(|segments| segments.last())
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.to_string())
                .unwrap_or_else(|| format!("download_{}.tmp", self.id))
        } else {
            format!("download_{}.tmp", self.id)
        };

        let _ = self.file_name.set(file_name.clone());
        file_name
    }

    pub(crate) fn get_or_init_file_path(
        &self,
        download_dir: &Path,
    ) -> Result<Arc<PathBuf>, DownloadError> {
        if let Some(existing_path) = self.file_path.get() {
            let file_path = Arc::new(existing_path.clone());
            debug!(
                "[Task {}] Using existing file path: {:?}",
                self.id, file_path
            );
            return Ok(file_path);
        }

        let file_name = self.resolve_or_init_file_name();
        let path = download_dir.join(&file_name);
        let _ = self.file_path.set(path);

        let file_path_ref = self
            .file_path
            .get()
            .ok_or_else(|| DownloadError::Other("file_path not set".into()))?;

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
            .finish()
    }
}
