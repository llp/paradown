use crate::checksum::DownloadChecksum;
use crate::status::DownloadStatus;

//---------------------------------------------------------------------------------
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTaskRequest {
    pub id: Option<u32>,
    pub url: String,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub checksums: Option<Vec<DownloadChecksum>>,
    pub status: Option<DownloadStatus>,
    pub downloaded_size: Option<u64>,
    pub total_size: Option<u64>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl DownloadTaskRequest {
    /// 新建 Builder，必须提供 URL
    pub fn builder(url: impl Into<String>) -> DownloadRequestBuilder {
        DownloadRequestBuilder {
            id: None,
            url: url.into(),
            file_name: None,
            file_path: None,
            checksums: Some(Vec::new()),
            status: None,
            downloaded_size: None,
            total_size: None,
            created_at: None,
            updated_at: None,
        }
    }
}

pub struct DownloadRequestBuilder {
    id: Option<u32>,
    url: String,
    file_name: Option<String>,
    file_path: Option<String>,
    checksums: Option<Vec<DownloadChecksum>>,
    status: Option<DownloadStatus>,
    downloaded_size: Option<u64>,
    total_size: Option<u64>,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
}

impl DownloadRequestBuilder {
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

    pub fn checksums(mut self, checksums: Vec<DownloadChecksum>) -> Self {
        self.checksums = Some(checksums);
        self
    }

    pub fn status(mut self, status: DownloadStatus) -> Self {
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

    pub fn created_at(mut self, created_at: DateTime<Utc>) -> Self {
        self.created_at = Some(created_at);
        self
    }

    pub fn updated_at(mut self, updated_at: DateTime<Utc>) -> Self {
        self.updated_at = Some(updated_at);
        self
    }

    /// 构建最终的 DownloadRequest
    pub fn build(self) -> DownloadTaskRequest {
        DownloadTaskRequest {
            id: self.id,
            url: self.url,
            file_name: self.file_name,
            file_path: self.file_path,
            checksums: self.checksums,
            status: self.status,
            downloaded_size: self.downloaded_size,
            total_size: self.total_size,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}
//---------------------------------------------------------------------------------

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct DownloadWorkerRequest {
    pub id: Option<u32>,
    pub task_id: u32,
    pub index: u32,
    pub start: u64,
    pub end: u64,
    pub downloaded: Option<u64>,
    pub status: Option<String>, // Pending / Running / Paused / Completed / Failed
    pub updated_at: Option<DateTime<Utc>>,
}

pub struct DownloadWorkerBuilder {
    id: Option<u32>,
    task_id: u32,
    index: u32,
    start: u64,
    end: u64,
    downloaded: Option<u64>,
    status: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}

impl DownloadWorkerBuilder {
    pub fn new(task_id: u32, index: u32, start: u64, end: u64) -> Self {
        Self {
            id: None,
            task_id,
            index,
            start,
            end,
            downloaded: None,
            status: None,
            updated_at: None,
        }
    }

    pub fn id(mut self, id: u32) -> Self {
        self.id = Some(id);
        self
    }

    pub fn downloaded(mut self, downloaded: u64) -> Self {
        self.downloaded = Some(downloaded);
        self
    }

    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn updated_at(mut self, dt: DateTime<Utc>) -> Self {
        self.updated_at = Some(dt);
        self
    }

    pub fn build(self) -> DownloadWorkerRequest {
        DownloadWorkerRequest {
            id: self.id,
            task_id: self.task_id,
            index: self.index,
            start: self.start,
            end: self.end,
            downloaded: self.downloaded,
            status: self.status,
            updated_at: self.updated_at,
        }
    }
}
