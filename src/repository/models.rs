use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DBDownloadTask {
    pub id: u32,
    pub url: String,
    pub resolved_url: String,
    pub entity_tag: String,
    pub last_modified: String,
    pub file_name: String,
    pub file_path: String,
    pub status: String, // Pending / Running / Paused / Completed / Failed
    pub downloaded_size: u64,
    pub total_size: Option<u64>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DBDownloadWorker {
    pub id: u32,
    pub task_id: u32,
    pub index: u32,
    pub start: u64,
    pub end: u64,
    pub downloaded: u64,
    pub status: String, // Pending / Running / Paused / Completed / Failed
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DBDownloadPiece {
    pub task_id: u32,
    pub piece_index: u32,
    pub completed: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DBDownloadChecksum {
    pub id: u32,
    pub task_id: u32,
    pub algorithm: String, // MD5 / SHA1 / SHA256
    pub value: String,
    pub verified: bool,
    pub verified_at: Option<DateTime<Utc>>,
}
