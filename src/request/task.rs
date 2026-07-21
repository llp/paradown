use crate::checksum::Checksum;
use crate::domain::{
    BlockState, DownloadSpec, HttpRequestOptions, HttpResourceIdentity, PieceState, SourceSet,
};
use crate::error::Error;
use crate::p2p::TorrentResumeSnapshot;
use crate::status::Status;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    pub id: Option<u32>,
    pub spec: DownloadSpec,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub resource_identity: Option<HttpResourceIdentity>,
    pub http_request: Option<HttpRequestOptions>,
    pub sources: Option<SourceSet>,
    pub piece_states: Option<Vec<PieceState>>,
    pub block_states: Option<Vec<BlockState>>,
    pub checksums: Option<Vec<Checksum>>,
    pub status: Option<Status>,
    pub downloaded_size: Option<u64>,
    pub total_size: Option<u64>,
    pub torrent_resume: Option<TorrentResumeSnapshot>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl TaskRequest {
    /// 创建 `TaskRequestBuilder`。
    ///
    /// 这里接收 `spec: DownloadSpec`，表示调用者把 `DownloadSpec` 的所有权交给 builder。
    /// builder 最后 `build(self)` 时会再把它移动进 `TaskRequest`。
    pub fn builder(spec: DownloadSpec) -> TaskRequestBuilder {
        TaskRequestBuilder {
            id: None,
            spec,
            file_name: None,
            file_path: None,
            resource_identity: None,
            http_request: None,
            sources: None,
            piece_states: None,
            block_states: None,
            checksums: Some(Vec::new()),
            status: None,
            downloaded_size: None,
            total_size: None,
            torrent_resume: None,
            created_at: None,
            updated_at: None,
        }
    }

    /// 从字符串定位符创建 builder。
    ///
    /// `locator: impl Into<String>` 是泛型参数简写，允许传入 `&str` 或 `String`。
    /// `DownloadSpec::parse(locator.into())?` 里的 `?` 会在解析失败时提前返回 `Err(Error)`。
    /// `Result` 和 `?` 的系统解释见 `docs/rust/result-option.md`。
    pub fn from_locator(locator: impl Into<String>) -> Result<TaskRequestBuilder, Error> {
        Ok(Self::builder(DownloadSpec::parse(locator.into())?))
    }

    pub fn locator(&self) -> &str {
        self.spec.locator()
    }
}

/// `TaskRequest` 的 builder。
///
/// builder 模式在 Rust 中常和 `mut self -> Self` 搭配：
/// 每个设置方法消费旧 builder、返回新 builder，最后 `build(self)` 消费 builder 生成目标值。
/// 这种方式避免复杂的生命周期借用，也让链式调用很自然。
pub struct TaskRequestBuilder {
    id: Option<u32>,
    spec: DownloadSpec,
    file_name: Option<String>,
    file_path: Option<String>,
    resource_identity: Option<HttpResourceIdentity>,
    http_request: Option<HttpRequestOptions>,
    sources: Option<SourceSet>,
    piece_states: Option<Vec<PieceState>>,
    block_states: Option<Vec<BlockState>>,
    checksums: Option<Vec<Checksum>>,
    status: Option<Status>,
    downloaded_size: Option<u64>,
    total_size: Option<u64>,
    torrent_resume: Option<TorrentResumeSnapshot>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
}

impl TaskRequestBuilder {
    /// 典型的 builder setter：取得 `mut self`，修改字段，返回 `Self`。
    pub fn id(mut self, id: u32) -> Self {
        self.id = Some(id);
        self
    }

    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    pub fn file_path(mut self, path: impl Into<String>) -> Self {
        self.file_path = Some(path.into());
        self
    }

    pub fn checksums(mut self, checksums: Vec<Checksum>) -> Self {
        self.checksums = Some(checksums);
        self
    }

    pub fn sources(mut self, sources: SourceSet) -> Self {
        self.sources = Some(sources);
        self
    }

    pub fn piece_states(mut self, piece_states: Vec<PieceState>) -> Self {
        self.piece_states = Some(piece_states);
        self
    }

    pub fn block_states(mut self, block_states: Vec<BlockState>) -> Self {
        self.block_states = Some(block_states);
        self
    }

    pub fn http_request(mut self, http_request: HttpRequestOptions) -> Self {
        self.http_request = Some(http_request);
        self
    }

    pub fn resource_identity(mut self, identity: HttpResourceIdentity) -> Self {
        self.resource_identity = Some(identity);
        self
    }

    pub fn status(mut self, status: Status) -> Self {
        self.status = Some(status);
        self
    }

    pub fn downloaded_size(mut self, size: u64) -> Self {
        self.downloaded_size = Some(size);
        self
    }

    pub fn total_size(mut self, size: u64) -> Self {
        self.total_size = Some(size);
        self
    }

    pub fn torrent_resume(mut self, resume: TorrentResumeSnapshot) -> Self {
        self.torrent_resume = Some(resume);
        self
    }

    pub fn created_at(mut self, created_at: DateTime<Utc>) -> Self {
        self.created_at = Some(created_at);
        self
    }

    pub fn updated_at(mut self, updated_at: DateTime<Utc>) -> Self {
        self.updated_at = Some(updated_at);
        self
    }

    /// `build(self)` 消费 builder。
    ///
    /// 因为 `self` 被消费，下面可以直接移动所有字段：
    /// `spec: self.spec`、`checksums: self.checksums` 等都不会产生复制。
    pub fn build(self) -> TaskRequest {
        TaskRequest {
            id: self.id,
            spec: self.spec,
            file_name: self.file_name,
            file_path: self.file_path,
            resource_identity: self.resource_identity,
            http_request: self.http_request,
            sources: self.sources,
            piece_states: self.piece_states,
            block_states: self.block_states,
            checksums: self.checksums,
            status: self.status,
            downloaded_size: self.downloaded_size,
            total_size: self.total_size,
            torrent_resume: self.torrent_resume,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}
