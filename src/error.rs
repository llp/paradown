use crate::config::DownloadConfigError;
use reqwest::header::ToStrError;
use std::error::Error as StdError;
use std::io;
use std::num::ParseIntError;
use thiserror::Error;
use tokio::sync::AcquireError;
use tokio::task::JoinError;


#[derive(Debug, Error, Clone)]
pub enum DownloadError {
    #[error("IO error: {0}")]
    Io(String),

    #[error("HTTP error: {0}")]
    Reqwest(String),

    #[error("HTTP error for file {0}: {1} {2}")]
    HttpError(u32, u16, String),

    #[error("Network error for file {0}: {1}")]
    NetworkError(u32, String),

    #[error("Checksum mismatch for file {0}: expected {1}, got {2}")]
    ChecksumMismatch(u32, String, String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("URL parse error: {0}")]
    UrlParseError(#[from] url::ParseError),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Other error: {0}")]
    Other(String),

    #[error("Task join error: {0}")]
    JoinError(String),

    #[error("Semaphore acquire error: {0}")]
    AcquireError(String),

    #[error("Task {0} not found")]
    TaskNotFound(u32),

    #[error("Task {0} is cancelled")]
    Canceled(u32),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Header ToStrError: {0}")]
    Header(String),
}

impl From<sqlx::Error> for DownloadError {
    fn from(err: sqlx::Error) -> Self {
        DownloadError::Other(err.to_string())
    }
}

impl From<io::Error> for DownloadError {
    fn from(err: io::Error) -> Self {
        DownloadError::Io(err.to_string())
    }
}

impl From<reqwest::Error> for DownloadError {
    fn from(err: reqwest::Error) -> Self {
        DownloadError::Reqwest(err.to_string())
    }
}

impl From<JoinError> for DownloadError {
    fn from(err: JoinError) -> Self {
        DownloadError::JoinError(err.to_string())
    }
}

impl From<AcquireError> for DownloadError {
    fn from(err: AcquireError) -> Self {
        DownloadError::AcquireError(err.to_string())
    }
}

impl From<DownloadConfigError> for DownloadError {
    fn from(err: DownloadConfigError) -> Self {
        DownloadError::ConfigError(err.to_string())
    }
}

impl From<Box<dyn StdError>> for DownloadError {
    fn from(err: Box<dyn StdError>) -> Self {
        DownloadError::Other(err.to_string())
    }
}

impl From<String> for DownloadError {
    fn from(s: String) -> Self {
        DownloadError::Other(s)
    }
}

impl From<&str> for DownloadError {
    fn from(s: &str) -> Self {
        DownloadError::Other(s.to_string())
    }
}

impl From<ParseIntError> for DownloadError {
    fn from(err: ParseIntError) -> Self {
        DownloadError::Parse(err.to_string())
    }
}

impl From<ToStrError> for DownloadError {
    fn from(err: ToStrError) -> Self {
        DownloadError::Header(err.to_string())
    }
}