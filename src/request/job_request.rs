use crate::checksum::DownloadChecksum;
use crate::status::DownloadStatus;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

pub type DownloadJobRequest = DownloadTaskRequest;

impl DownloadTaskRequest {
    pub fn builder(url: impl Into<String>) -> DownloadJobRequestBuilder {
        DownloadJobRequestBuilder {
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

pub struct DownloadJobRequestBuilder {
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

pub type DownloadRequestBuilder = DownloadJobRequestBuilder;

impl DownloadJobRequestBuilder {
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
