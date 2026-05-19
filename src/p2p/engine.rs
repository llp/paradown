use crate::domain::{DownloadSpec, SessionManifest};
use crate::error::Error;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TorrentEngineBackend {
    Libtorrent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentEngineCapabilities {
    pub backend: TorrentEngineBackend,
    pub available: bool,
    pub torrent_files: bool,
    pub magnets: bool,
    pub metadata_exchange: bool,
    pub trackers: bool,
    pub dht: bool,
    pub peer_exchange: bool,
    pub utp: bool,
    pub web_seeds: bool,
    pub fast_resume: bool,
    pub file_priorities: bool,
}

impl TorrentEngineCapabilities {
    pub fn libtorrent_full() -> Self {
        Self {
            backend: TorrentEngineBackend::Libtorrent,
            available: true,
            torrent_files: true,
            magnets: true,
            metadata_exchange: true,
            trackers: true,
            dht: true,
            peer_exchange: true,
            utp: true,
            web_seeds: true,
            fast_resume: true,
            file_priorities: true,
        }
    }

    pub fn libtorrent_unavailable() -> Self {
        Self {
            available: false,
            ..Self::libtorrent_full()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentPieceHash {
    pub piece_index: u32,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentFileEntry {
    pub path_components: Vec<String>,
    pub length: u64,
    pub offset: u64,
}

impl TorrentFileEntry {
    pub fn display_path(&self) -> PathBuf {
        self.path_components.iter().collect()
    }

    pub fn file_name(&self) -> String {
        self.path_components
            .last()
            .cloned()
            .unwrap_or_else(|| "payload.bin".into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentTracker {
    pub url: String,
    pub tier: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentMetadata {
    pub name: String,
    pub info_hash_v1: Option<String>,
    pub info_hash_v2: Option<String>,
    pub piece_size: u32,
    pub piece_count: u32,
    pub total_size: u64,
    pub private: bool,
    pub files: Vec<TorrentFileEntry>,
    pub piece_hashes: Vec<TorrentPieceHash>,
    pub trackers: Vec<TorrentTracker>,
    pub web_seeds: Vec<String>,
}

impl TorrentMetadata {
    pub fn stable_id(&self) -> String {
        self.info_hash_v2
            .as_ref()
            .or(self.info_hash_v1.as_ref())
            .cloned()
            .unwrap_or_else(|| self.name.clone())
    }

    pub fn is_multi_file(&self) -> bool {
        self.files.len() > 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentEngineHandle {
    pub backend: TorrentEngineBackend,
    pub external_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TorrentEngineState {
    ResolvingMetadata,
    CheckingFiles,
    Downloading,
    Seeding,
    Paused,
    Completed,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TorrentEngineEvent {
    MetadataDiscovered(TorrentMetadata),
    StateChanged(TorrentEngineState),
    Progress {
        downloaded: u64,
        total: u64,
        download_rate_bps: u64,
        upload_rate_bps: u64,
        connected_peers: u32,
        seeds: u32,
    },
    PieceFinished {
        piece_index: u32,
    },
    ResumeData {
        bytes: Vec<u8>,
    },
    Finished,
    Error(String),
}

#[derive(Debug)]
pub struct TorrentEngineRequest {
    pub session_id: u32,
    pub spec: DownloadSpec,
    pub download_dir: PathBuf,
    pub requested_file_name: Option<String>,
    pub requested_file_path: Option<PathBuf>,
    pub rate_limit_kib_per_sec: Option<u64>,
    pub resume: Option<TorrentResumeSnapshot>,
    pub event_sender: Option<mpsc::UnboundedSender<TorrentEngineEvent>>,
}

#[derive(Debug, Clone)]
pub struct TorrentEngineSession {
    pub handle: TorrentEngineHandle,
    pub state: TorrentEngineState,
    pub metadata: Option<TorrentMetadata>,
    pub manifest: Option<SessionManifest>,
    pub resume_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentResumeSnapshot {
    pub handle: TorrentEngineHandle,
    pub state: TorrentEngineState,
    pub metadata: Option<TorrentMetadata>,
    pub resume_data: Option<Vec<u8>>,
}

impl TorrentResumeSnapshot {
    pub fn from_session(session: &TorrentEngineSession) -> Self {
        Self {
            handle: session.handle.clone(),
            state: session.state.clone(),
            metadata: session.metadata.clone(),
            resume_data: session.resume_data.clone(),
        }
    }
}

#[async_trait]
pub trait TorrentEngine: Send + Sync {
    fn backend(&self) -> TorrentEngineBackend;

    fn capabilities(&self) -> TorrentEngineCapabilities;

    async fn start_session(
        &self,
        request: TorrentEngineRequest,
    ) -> Result<TorrentEngineSession, Error>;

    async fn pause_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error>;

    async fn resume_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error>;

    async fn cancel_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error>;

    async fn remove_session(
        &self,
        handle: &TorrentEngineHandle,
        delete_payload: bool,
    ) -> Result<(), Error>;
}
