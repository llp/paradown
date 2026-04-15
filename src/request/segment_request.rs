use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct DownloadWorkerRequest {
    pub id: Option<u32>,
    pub task_id: u32,
    pub index: u32,
    pub start: u64,
    pub end: u64,
    pub downloaded: Option<u64>,
    pub status: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub type SegmentRequest = DownloadWorkerRequest;

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

pub type SegmentRequestBuilder = DownloadWorkerBuilder;

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
