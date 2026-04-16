pub(crate) mod finalize;
pub(crate) mod prepare;
pub(crate) mod state;
pub(crate) mod storage;
pub(crate) mod workers;

use self::prepare::{PreparationOutcome, prepare_download};
use self::state::StartDirective;
use crate::checksum::Checksum;
use crate::config::Config;
use crate::coordinator::Manager;
use crate::domain::{
    BlockState, DownloadSpec, HttpRequestOptions, HttpResourceIdentity, PieceState,
    SessionManifest, SourceSet, completed_block_count, completed_piece_count,
    derive_piece_states_from_blocks, file_name_hint_from_locator, initialize_block_states,
    initialize_piece_states, mark_completed_blocks,
};
use crate::error::Error;
use crate::events::Event;
use crate::payload::store::PayloadStore;
use crate::stats::{Stats, StatsSnapshot};
use crate::status::Status;
use crate::storage::Store;
use crate::worker::Worker;
use chrono::{DateTime, Utc};
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::{Mutex, OnceCell, OwnedSemaphorePermit, RwLock, broadcast};

pub struct Task {
    pub id: u32,
    trace_id: String,
    pub spec: DownloadSpec,
    pub status: Mutex<Status>,
    pub file_name: OnceCell<String>,
    pub file_path: Arc<OnceCell<PathBuf>>,
    http_resource_identity: RwLock<HttpResourceIdentity>,
    http_request: HttpRequestOptions,
    sources: RwLock<SourceSet>,
    source_runtime: RwLock<HashMap<String, SourceRuntimeState>>,
    pub checksums: Mutex<Vec<Checksum>>,
    manifest: RwLock<Option<SessionManifest>>,
    piece_states: RwLock<Vec<PieceState>>,
    block_states: RwLock<Vec<BlockState>>,
    payload_store: RwLock<Option<Arc<PayloadStore>>>,
    pub config: Arc<Config>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Mutex<Option<DateTime<Utc>>>,
    pub persistence: Option<Arc<Store>>,
    pub workers: RwLock<Vec<Arc<Worker>>>,

    pub total_size: AtomicU64,
    pub downloaded_size: AtomicU64,
    pub range_requests_supported: AtomicBool,
    pub protocol_probe_completed: AtomicBool,
    pub total_size_known: AtomicBool,

    pub worker_event_tx: broadcast::Sender<Event>,
    pub manager: Weak<Manager>,
    pub stats: Arc<Stats>,
    pub client: Arc<reqwest::Client>,
    pub permit: Mutex<Option<OwnedSemaphorePermit>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: u32,
    pub trace_id: String,
    pub url: String,
    pub file_name: Option<String>,
    pub file_path: Option<PathBuf>,
    pub status: String,
    pub downloaded_size: u64,
    pub total_size: u64,
    pub total_size_known: bool,
    pub source_count: usize,
    pub primary_source_locator: Option<String>,
    pub source_locators: Vec<String>,
    pub completed_pieces: u32,
    pub piece_count: u32,
    pub completed_blocks: u32,
    pub block_count: u32,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub checksums: Vec<Checksum>,
    pub stats: StatsSnapshot,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PieceProgressSnapshot {
    pub downloaded: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SourceRuntimeState {
    pub consecutive_failures: u32,
    pub successful_transfers: u32,
}

impl Task {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u32,
        spec: DownloadSpec,
        file_name: Option<String>,
        file_path: Option<String>,
        resource_identity: Option<HttpResourceIdentity>,
        http_request: HttpRequestOptions,
        sources: Option<SourceSet>,
        piece_states: Option<Vec<PieceState>>,
        block_states: Option<Vec<BlockState>>,
        status: Option<Status>,
        downloaded_size: Option<u64>,
        total_size: Option<u64>,
        checksums: Vec<Checksum>,
        client: Arc<reqwest::Client>,
        config: Arc<Config>,
        persistence: Option<Arc<Store>>,
        manager: Weak<Manager>,
        created_at: Option<DateTime<Utc>>,
        updated_at: Option<DateTime<Utc>>,
    ) -> Result<Arc<Self>, Error> {
        let (worker_event_tx, _) = broadcast::channel(100);

        let file_name_cell = OnceCell::new();
        if let Some(name) = file_name {
            let _ = file_name_cell.set(name);
        }

        let file_path_cell = Arc::new(OnceCell::new());
        if let Some(path) = file_path {
            let _ = file_path_cell.set(PathBuf::from(path));
        }

        let initial_status = status.unwrap_or(Status::Pending);
        let now = Utc::now();
        let initial_sources =
            sources.unwrap_or_else(|| SourceSet::for_spec(&spec, Some(http_request.clone())));

        Ok(Arc::new(Self {
            id,
            trace_id: format!("task-{id:06}"),
            spec,
            file_name: file_name_cell,
            file_path: file_path_cell,
            http_resource_identity: RwLock::new(resource_identity.unwrap_or_default()),
            http_request,
            sources: RwLock::new(initial_sources),
            source_runtime: RwLock::new(HashMap::new()),
            checksums: Mutex::new(checksums),
            manifest: RwLock::new(None),
            piece_states: RwLock::new(piece_states.unwrap_or_default()),
            block_states: RwLock::new(block_states.unwrap_or_default()),
            payload_store: RwLock::new(None),
            status: Mutex::new(initial_status),
            downloaded_size: AtomicU64::new(downloaded_size.unwrap_or(0)),
            config,
            total_size: AtomicU64::new(total_size.unwrap_or(0)),
            range_requests_supported: AtomicBool::new(false),
            protocol_probe_completed: AtomicBool::new(false),
            total_size_known: AtomicBool::new(total_size.is_some()),
            created_at: Some(created_at.unwrap_or(now)),
            updated_at: Mutex::new(Some(updated_at.unwrap_or(now))),
            persistence,
            workers: RwLock::new(vec![]),
            worker_event_tx,
            manager,
            stats: Arc::new(Stats::new()),
            client,
            permit: Mutex::new(None),
        }))
    }

    pub async fn snapshot(&self) -> TaskSnapshot {
        let status_guard = self.status.lock().await;
        let status_str = match &*status_guard {
            Status::Failed(err) => format!("Failed: {}", err),
            _ => status_guard.to_string(),
        };

        let updated_at_guard = self.updated_at.lock().await;
        let piece_states = self.piece_states.read().await;
        let completed_pieces = completed_piece_count(&piece_states);
        let piece_count = piece_states.len() as u32;
        let block_states = self.block_states.read().await;
        let completed_blocks = completed_block_count(&block_states);
        let block_count = block_states.len() as u32;
        let stats = self.stats.snapshot().await;
        let sources = self.sources.read().await.clone();
        TaskSnapshot {
            id: self.id,
            trace_id: self.trace_id.clone(),
            url: self.spec.locator().to_string(),
            file_name: self.file_name.get().cloned(),
            file_path: self.file_path.get().cloned(),
            status: status_str,
            downloaded_size: self.downloaded_size.load(Ordering::Relaxed),
            total_size: self.total_size.load(Ordering::Relaxed),
            total_size_known: self.total_size_known.load(Ordering::Relaxed),
            source_count: sources.len(),
            primary_source_locator: sources.primary().map(|source| source.locator.clone()),
            source_locators: sources
                .sources
                .into_iter()
                .map(|source| source.locator)
                .collect(),
            completed_pieces,
            piece_count,
            completed_blocks,
            block_count,
            created_at: self.created_at,
            updated_at: *updated_at_guard,
            checksums: self.checksums.lock().await.clone(),
            stats,
        }
    }

    pub async fn init(self: &Arc<Self>) -> Result<(), Error> {
        debug!("[Task {}] Initializing task: {}", self.id, self.spec);

        self.persist_task().await?;
        self.persist_task_checksums().await?;
        self.spawn_worker_event_listener();
        self.emit_manager_event(Event::Pending(self.id));

        Ok(())
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), Error> {
        let StartDirective::Continue = self.prepare_for_start().await?;

        self.set_status(Status::Preparing).await;
        debug!("[Task {}] Preparing download", self.id);
        self.emit_manager_event(Event::Preparing(self.id));
        self.persist_task().await?;

        let file_path = match prepare_download(self).await? {
            PreparationOutcome::Ready(prepared) => prepared.file_path,
            PreparationOutcome::Finished => return Ok(()),
        };

        self.set_status(Status::Running).await;
        debug!("[Task {}] Starting download", self.id);
        self.emit_manager_event(Event::Start(self.id));
        self.persist_task().await?;

        let workers = self.ensure_workers(&file_path).await?;
        self.spawn_workers(workers);

        Ok(())
    }

    pub(crate) fn emit_manager_event(&self, event: Event) {
        if let Some(manager) = self.manager.upgrade() {
            let _ = manager.task_event_tx.send(event);
        }
    }

    pub(crate) fn update_protocol_probe(
        &self,
        total_size: Option<u64>,
        supports_range_requests: bool,
    ) {
        self.total_size
            .store(total_size.unwrap_or(0), Ordering::Relaxed);
        self.total_size_known
            .store(total_size.is_some(), Ordering::Relaxed);
        self.range_requests_supported
            .store(supports_range_requests, Ordering::Relaxed);
        self.protocol_probe_completed.store(true, Ordering::Relaxed);
    }

    pub(crate) fn supports_range_requests(&self) -> bool {
        self.range_requests_supported.load(Ordering::Relaxed)
    }

    pub(crate) fn total_size_option(&self) -> Option<u64> {
        self.total_size_known
            .load(Ordering::Relaxed)
            .then(|| self.total_size.load(Ordering::Relaxed))
    }

    pub(crate) fn finalize_stream_total_size(&self, total_size: u64) {
        self.total_size.store(total_size, Ordering::Relaxed);
        self.total_size_known.store(true, Ordering::Relaxed);
    }

    pub(crate) fn resolve_or_init_file_name(&self) -> String {
        if let Some(name) = self.file_name.get() {
            return name.clone();
        }

        let file_name = self
            .http_resource_identity
            .try_read()
            .ok()
            .and_then(|identity| identity.resolved_url.clone())
            .and_then(|url| file_name_hint_from_locator(&url))
            .or_else(|| self.spec.file_name_hint())
            .unwrap_or_else(|| format!("download_{}.tmp", self.id));

        let _ = self.file_name.set(file_name.clone());
        file_name
    }

    pub(crate) fn initialize_file_name(&self, suggested_file_name: Option<String>) -> String {
        if let Some(file_name) = self.file_name.get() {
            return file_name.clone();
        }

        let file_name = suggested_file_name
            .or_else(|| {
                self.http_resource_identity
                    .try_read()
                    .ok()
                    .and_then(|identity| identity.resolved_url.clone())
                    .and_then(|url| file_name_hint_from_locator(&url))
            })
            .or_else(|| self.spec.file_name_hint())
            .unwrap_or_else(|| format!("download_{}.tmp", self.id));
        let _ = self.file_name.set(file_name.clone());
        file_name
    }

    pub(crate) fn get_or_init_file_path(&self, download_dir: &Path) -> Result<Arc<PathBuf>, Error> {
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
            .ok_or_else(|| Error::Other("file_path not set".into()))?;

        let file_path = Arc::new(file_path_ref.clone());
        debug!("[Task {}] Initialized file path: {:?}", self.id, file_path);

        Ok(file_path)
    }

    pub(crate) async fn install_manifest(
        self: &Arc<Self>,
        manifest: SessionManifest,
    ) -> Arc<PayloadStore> {
        let payload_store = Arc::new(PayloadStore::new(&manifest));
        let manifest_sources = manifest.sources.clone();
        let existing_piece_states = self.piece_states.read().await.clone();
        let existing_block_states = self.block_states.read().await.clone();
        let mut piece_states = initialize_piece_states(&manifest.pieces);
        let mut block_states = initialize_block_states(&manifest.blocks);
        if piece_state_layout_matches(&piece_states, &existing_piece_states) {
            for (next, restored) in piece_states.iter_mut().zip(existing_piece_states.iter()) {
                next.completed = restored.completed;
            }
        }
        if block_state_layout_matches(&block_states, &existing_block_states) {
            for (next, restored) in block_states.iter_mut().zip(existing_block_states.iter()) {
                next.completed = restored.completed;
            }
            piece_states =
                derive_piece_states_from_blocks(&manifest.pieces, &manifest.blocks, &block_states);
        }
        *self.manifest.write().await = Some(manifest);
        *self.sources.write().await = manifest_sources;
        *self.piece_states.write().await = piece_states;
        *self.block_states.write().await = block_states;
        *self.payload_store.write().await = Some(Arc::clone(&payload_store));
        payload_store
    }

    pub(crate) async fn payload_store(&self) -> Result<Arc<PayloadStore>, Error> {
        self.payload_store
            .read()
            .await
            .clone()
            .ok_or_else(|| Error::Other(format!("Task {} payload store not initialized", self.id)))
    }

    pub(crate) async fn clear_manifest_state(&self) {
        *self.manifest.write().await = None;
        self.piece_states.write().await.clear();
        self.block_states.write().await.clear();
        *self.payload_store.write().await = None;
    }

    pub(crate) async fn http_resource_identity(&self) -> HttpResourceIdentity {
        self.http_resource_identity.read().await.clone()
    }

    pub(crate) fn http_request_options(&self) -> &HttpRequestOptions {
        &self.http_request
    }

    pub(crate) fn trace_id(&self) -> &str {
        &self.trace_id
    }

    pub(crate) async fn set_http_resource_identity(&self, identity: HttpResourceIdentity) {
        *self.http_resource_identity.write().await = identity;
    }

    pub(crate) async fn piece_states_snapshot(&self) -> Vec<PieceState> {
        self.piece_states.read().await.clone()
    }

    pub(crate) async fn block_states_snapshot(&self) -> Vec<BlockState> {
        self.block_states.read().await.clone()
    }

    pub(crate) async fn source_set_snapshot(&self) -> SourceSet {
        self.sources.read().await.clone()
    }

    pub(crate) async fn set_source_set(&self, source_set: SourceSet) {
        *self.sources.write().await = source_set;
    }

    pub(crate) async fn record_source_success(&self, source_id: &str) {
        let mut runtime = self.source_runtime.write().await;
        let state = runtime.entry(source_id.to_string()).or_default();
        state.successful_transfers = state.successful_transfers.saturating_add(1);
        state.consecutive_failures = 0;
    }

    pub(crate) async fn record_source_failure(&self, source_id: &str) {
        let mut runtime = self.source_runtime.write().await;
        let state = runtime.entry(source_id.to_string()).or_default();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    }

    pub(crate) async fn ranked_transfer_sources(&self) -> Vec<crate::domain::SourceDescriptor> {
        let source_set = self.source_set_snapshot().await;
        let primary_id = source_set.primary().map(|source| source.id.clone());
        let runtime = self.source_runtime.read().await;
        let mut sources = source_set
            .active_transfer_sources()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();

        sources.sort_by(|left, right| {
            let left_state = runtime.get(&left.id).cloned().unwrap_or_default();
            let right_state = runtime.get(&right.id).cloned().unwrap_or_default();

            left_state
                .consecutive_failures
                .cmp(&right_state.consecutive_failures)
                .then_with(|| {
                    right_state
                        .successful_transfers
                        .cmp(&left_state.successful_transfers)
                })
                .then_with(|| {
                    let left_is_primary = primary_id.as_deref() == Some(left.id.as_str());
                    let right_is_primary = primary_id.as_deref() == Some(right.id.as_str());
                    right_is_primary.cmp(&left_is_primary)
                })
                .then_with(|| left.id.cmp(&right.id))
        });

        sources
    }

    pub(crate) async fn preferred_transfer_source_ids(&self) -> Vec<String> {
        self.ranked_transfer_sources()
            .await
            .into_iter()
            .map(|source| source.id)
            .collect()
    }

    pub(crate) async fn select_retry_source(
        &self,
        current_source_id: &str,
    ) -> Option<crate::domain::SourceDescriptor> {
        let ranked = self.ranked_transfer_sources().await;
        let current_state = self
            .source_runtime
            .read()
            .await
            .get(current_source_id)
            .cloned()
            .unwrap_or_default();
        if current_state.consecutive_failures == 0 {
            return None;
        }

        ranked
            .into_iter()
            .find(|source| source.id != current_source_id)
    }

    pub(crate) async fn reset_piece_progress(&self) {
        let mut piece_states = self.piece_states.write().await;
        for piece in piece_states.iter_mut() {
            piece.completed = false;
        }
    }

    pub(crate) async fn reset_block_progress(&self) {
        let mut block_states = self.block_states.write().await;
        for block in block_states.iter_mut() {
            block.completed = false;
        }
    }

    pub(crate) async fn resume_validator(&self) -> Option<String> {
        self.http_resource_identity
            .read()
            .await
            .resume_validator()
            .map(str::to_string)
    }

    pub(crate) async fn sync_piece_progress_from_workers(
        &self,
    ) -> Result<PieceProgressSnapshot, Error> {
        let manifest =
            self.manifest.read().await.clone().ok_or_else(|| {
                Error::Other(format!("Task {} manifest not initialized", self.id))
            })?;
        let workers = self.workers.read().await.clone();

        let existing_block_states = self.block_states.read().await.clone();
        let mut block_states = if block_state_layout_matches(
            &initialize_block_states(&manifest.blocks),
            &existing_block_states,
        ) {
            existing_block_states
        } else {
            initialize_block_states(&manifest.blocks)
        };
        let mut downloaded = 0u64;

        for worker in workers {
            let worker_downloaded = worker
                .downloaded_size
                .load(Ordering::Relaxed)
                .min(worker.expected_length());
            downloaded = downloaded.saturating_add(worker_downloaded);

            if worker_downloaded == 0 {
                continue;
            }

            let covered_end = worker
                .start
                .saturating_add(worker_downloaded)
                .saturating_sub(1);
            mark_completed_blocks(
                &mut block_states,
                &manifest.blocks,
                worker.start,
                covered_end,
            );
        }

        let piece_states =
            derive_piece_states_from_blocks(&manifest.pieces, &manifest.blocks, &block_states);
        if !manifest.total_size_known {
            self.downloaded_size.store(downloaded, Ordering::Relaxed);
            return Ok(PieceProgressSnapshot { downloaded });
        }

        let downloaded = downloaded.min(manifest.total_size);
        *self.block_states.write().await = block_states;
        *self.piece_states.write().await = piece_states;
        self.downloaded_size.store(downloaded, Ordering::Relaxed);

        Ok(PieceProgressSnapshot { downloaded })
    }

    pub(crate) async fn mark_all_pieces_completed(&self) {
        let manifest = self.manifest.read().await.clone();
        if let Some(manifest) = manifest {
            let mut piece_states = self.piece_states.write().await;
            let mut block_states = self.block_states.write().await;
            if piece_states.len() != manifest.pieces.len() {
                *piece_states = initialize_piece_states(&manifest.pieces);
            }
            for piece_state in piece_states.iter_mut() {
                piece_state.completed = true;
            }
            if block_states.len() != manifest.blocks.len() {
                *block_states = initialize_block_states(&manifest.blocks);
            }
            for block_state in block_states.iter_mut() {
                block_state.completed = true;
            }
            if manifest.total_size_known {
                self.downloaded_size
                    .store(manifest.total_size, Ordering::Relaxed);
            }
        }
    }
}

fn piece_state_layout_matches(expected: &[PieceState], restored: &[PieceState]) -> bool {
    expected.len() == restored.len()
        && expected
            .iter()
            .zip(restored.iter())
            .all(|(expected, restored)| expected.piece_index == restored.piece_index)
}

fn block_state_layout_matches(expected: &[BlockState], restored: &[BlockState]) -> bool {
    expected.len() == restored.len()
        && expected
            .iter()
            .zip(restored.iter())
            .all(|(expected, restored)| {
                expected.piece_index == restored.piece_index
                    && expected.block_index == restored.block_index
            })
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("spec", &self.spec)
            .field("status", &self.status)
            .field("file_name", &self.file_name)
            .field("file_path", &self.file_path)
            .field("http_resource_identity", &self.http_resource_identity)
            .field("checksums", &self.checksums)
            .field("manifest", &self.manifest)
            .field("piece_states", &self.piece_states)
            .field("payload_store", &self.payload_store)
            .field("downloaded_size", &self.downloaded_size)
            .field("config", &self.config)
            .field("workers", &self.workers)
            .field("total_size", &self.total_size)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}
