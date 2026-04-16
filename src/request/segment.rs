use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct SegmentRequest {
    pub id: Option<u32>,
    pub task_id: u32,
    pub index: u32,
    pub source_id: Option<String>,
    pub piece_start: Option<u32>,
    pub piece_end: Option<u32>,
    pub block_start: Option<u32>,
    pub block_end: Option<u32>,
    pub start: u64,
    pub end: u64,
    pub downloaded: Option<u64>,
    pub status: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub struct SegmentRequestBuilder {
    id: Option<u32>,
    task_id: u32,
    index: u32,
    source_id: Option<String>,
    piece_start: Option<u32>,
    piece_end: Option<u32>,
    block_start: Option<u32>,
    block_end: Option<u32>,
    start: u64,
    end: u64,
    downloaded: Option<u64>,
    status: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}

impl SegmentRequestBuilder {
    pub fn new(task_id: u32, index: u32, start: u64, end: u64) -> Self {
        Self {
            id: None,
            task_id,
            index,
            source_id: None,
            piece_start: None,
            piece_end: None,
            block_start: None,
            block_end: None,
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

    pub fn source_id(mut self, source_id: impl Into<String>) -> Self {
        self.source_id = Some(source_id.into());
        self
    }

    pub fn piece_range(mut self, piece_start: u32, piece_end: u32) -> Self {
        self.piece_start = Some(piece_start);
        self.piece_end = Some(piece_end);
        self
    }

    pub fn block_range(mut self, block_start: u32, block_end: u32) -> Self {
        self.block_start = Some(block_start);
        self.block_end = Some(block_end);
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

    pub fn build(self) -> SegmentRequest {
        SegmentRequest {
            id: self.id,
            task_id: self.task_id,
            index: self.index,
            source_id: self.source_id,
            piece_start: self.piece_start,
            piece_end: self.piece_end,
            block_start: self.block_start,
            block_end: self.block_end,
            start: self.start,
            end: self.end,
            downloaded: self.downloaded,
            status: self.status,
            updated_at: self.updated_at,
        }
    }
}
