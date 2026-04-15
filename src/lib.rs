mod chunk;
mod runtime;

pub mod checksum;
pub mod cli;
pub mod config;
pub mod download;
pub mod error;
pub mod events;
pub mod manager;
pub mod persistence;
pub mod progress;
pub mod repository;
pub mod request;
pub mod stats;
pub mod status;
pub mod storage;
pub mod task;
pub mod worker;

pub use config::DownloadConfig;
pub use error::DownloadError;
pub use stats::DownloadStats;
pub use status::DownloadStatus;

pub use events::DownloadEvent;
pub use manager::{DownloadCoordinator, DownloadManager};
pub use persistence::{DownloadStore, StorageBackend};
pub use request::{DownloadJobRequest, DownloadTaskRequest, SegmentRequest};
pub use runtime::init_logger;
pub use task::{DownloadJob, DownloadJobSnapshot, DownloadTaskSnapshot};
pub use worker::SegmentWorker;

pub use config::{
    DownloadConfigBuilder, FileConflictStrategy, ProgressThrottleConfig, RetryConfig,
};
pub use persistence::PersistenceType;
