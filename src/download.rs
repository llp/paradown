use crate::checksum::Checksum;
pub use crate::coordinator::Manager;
pub use crate::domain::{
    BlockState, DownloadSpec, FileManifest, HttpResourceIdentity, PieceBlock, PieceLayout,
    PieceState, SessionDescriptor, SessionManifest, SessionMode, SourceCapabilities,
    SourceDescriptor, SourceKind, SourceSet,
};
use crate::error::Error;
pub use crate::events::Event;
use crate::job::{Task, TaskSnapshot};
pub use crate::request::{SegmentRequest, SegmentRequestBuilder};
use crate::request::{TaskRequest, TaskRequestBuilder};
pub use crate::stats::StatsSnapshot;
pub use crate::status::Status;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub use crate::worker::Worker;

#[derive(Clone)]
pub struct Session {
    inner: Arc<Task>,
}

impl Session {
    pub(crate) fn from_task(task: Arc<Task>) -> Self {
        Self { inner: task }
    }

    pub fn id(&self) -> u32 {
        self.inner.id
    }

    pub fn locator(&self) -> &str {
        self.inner.spec.locator()
    }

    pub async fn snapshot(&self) -> SessionSnapshot {
        self.inner.snapshot().await.into()
    }

    pub async fn pause(&self) -> Result<(), Error> {
        self.inner.pause().await
    }

    pub async fn resume(&self) -> Result<(), Error> {
        self.inner.resume().await
    }

    pub async fn cancel(&self) -> Result<(), Error> {
        self.inner.cancel().await
    }

    pub async fn delete(&self) -> Result<(), Error> {
        self.inner.delete().await
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.inner.id)
            .field("locator", &self.inner.spec.locator())
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub id: u32,
    pub trace_id: String,
    pub locator: String,
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

impl From<TaskSnapshot> for SessionSnapshot {
    fn from(snapshot: TaskSnapshot) -> Self {
        Self {
            id: snapshot.id,
            trace_id: snapshot.trace_id,
            locator: snapshot.url,
            file_name: snapshot.file_name,
            file_path: snapshot.file_path,
            status: snapshot.status,
            downloaded_size: snapshot.downloaded_size,
            total_size: snapshot.total_size,
            total_size_known: snapshot.total_size_known,
            source_count: snapshot.source_count,
            primary_source_locator: snapshot.primary_source_locator,
            source_locators: snapshot.source_locators,
            completed_pieces: snapshot.completed_pieces,
            piece_count: snapshot.piece_count,
            completed_blocks: snapshot.completed_blocks,
            block_count: snapshot.block_count,
            created_at: snapshot.created_at,
            updated_at: snapshot.updated_at,
            checksums: snapshot.checksums,
            stats: snapshot.stats,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionRequest {
    inner: TaskRequest,
}

impl SessionRequest {
    pub fn builder(spec: DownloadSpec) -> SessionRequestBuilder {
        SessionRequestBuilder {
            inner: TaskRequest::builder(spec),
        }
    }

    pub fn from_locator(locator: impl Into<String>) -> Result<SessionRequestBuilder, Error> {
        Ok(SessionRequestBuilder {
            inner: TaskRequest::from_locator(locator)?,
        })
    }

    pub fn locator(&self) -> &str {
        self.inner.locator()
    }

    pub(crate) fn into_inner(self) -> TaskRequest {
        self.inner
    }
}

pub struct SessionRequestBuilder {
    inner: TaskRequestBuilder,
}

impl SessionRequestBuilder {
    pub fn id(mut self, id: u32) -> Self {
        self.inner = self.inner.id(id);
        self
    }

    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.inner = self.inner.file_name(name);
        self
    }

    pub fn file_path(mut self, path: impl Into<String>) -> Self {
        self.inner = self.inner.file_path(path);
        self
    }

    pub fn checksums(mut self, checksums: Vec<Checksum>) -> Self {
        self.inner = self.inner.checksums(checksums);
        self
    }

    pub fn sources(mut self, sources: SourceSet) -> Self {
        self.inner = self.inner.sources(sources);
        self
    }

    pub fn piece_states(mut self, piece_states: Vec<PieceState>) -> Self {
        self.inner = self.inner.piece_states(piece_states);
        self
    }

    pub fn block_states(mut self, block_states: Vec<BlockState>) -> Self {
        self.inner = self.inner.block_states(block_states);
        self
    }

    pub fn http_request(mut self, http_request: crate::domain::HttpRequestOptions) -> Self {
        self.inner = self.inner.http_request(http_request);
        self
    }

    pub fn resource_identity(mut self, identity: HttpResourceIdentity) -> Self {
        self.inner = self.inner.resource_identity(identity);
        self
    }

    pub fn status(mut self, status: Status) -> Self {
        self.inner = self.inner.status(status);
        self
    }

    pub fn downloaded_size(mut self, size: u64) -> Self {
        self.inner = self.inner.downloaded_size(size);
        self
    }

    pub fn total_size(mut self, size: u64) -> Self {
        self.inner = self.inner.total_size(size);
        self
    }

    pub fn created_at(mut self, created_at: DateTime<Utc>) -> Self {
        self.inner = self.inner.created_at(created_at);
        self
    }

    pub fn updated_at(mut self, updated_at: DateTime<Utc>) -> Self {
        self.inner = self.inner.updated_at(updated_at);
        self
    }

    pub fn build(self) -> SessionRequest {
        SessionRequest {
            inner: self.inner.build(),
        }
    }
}
