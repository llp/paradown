use crate::config::{ConfigError, ConfigLoadError};
use reqwest::header::ToStrError;
use serde::{Deserialize, Serialize};
use std::error::Error as StdError;
use std::io;
use std::num::ParseIntError;
use thiserror::Error;
use tokio::sync::AcquireError;
use tokio::task::JoinError;

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(String),

    #[error("HTTP error: {0}")]
    Reqwest(String),

    #[error("HTTP error for file {0}: {1} {2}")]
    HttpError(u32, u16, String),

    #[error("Network error for file {0}: {1}")]
    NetworkError(u32, String),

    #[error("Resume invalidated for file {0}: {1}")]
    ResumeInvalidated(u32, String),

    #[error("Checksum mismatch for file {0}: expected {1}, got {2}")]
    ChecksumMismatch(u32, String, String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Unsupported protocol: {0}")]
    UnsupportedProtocol(String),

    #[error("URL parse error: {0}")]
    UrlParseError(String),

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

// 手动实现转换，从 url::ParseError -> Error::UrlParseError
impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Self {
        Error::UrlParseError(err.to_string())
    }
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::Other(err.to_string())
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Error::Reqwest(err.to_string())
    }
}

impl From<JoinError> for Error {
    fn from(err: JoinError) -> Self {
        Error::JoinError(err.to_string())
    }
}

impl From<AcquireError> for Error {
    fn from(err: AcquireError) -> Self {
        Error::AcquireError(err.to_string())
    }
}

impl From<ConfigError> for Error {
    fn from(err: ConfigError) -> Self {
        Error::ConfigError(err.to_string())
    }
}

impl From<ConfigLoadError> for Error {
    fn from(err: ConfigLoadError) -> Self {
        Error::ConfigError(err.to_string())
    }
}

impl From<Box<dyn StdError>> for Error {
    fn from(err: Box<dyn StdError>) -> Self {
        Error::Other(err.to_string())
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Other(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::Other(s.to_string())
    }
}

impl From<ParseIntError> for Error {
    fn from(err: ParseIntError) -> Self {
        Error::Parse(err.to_string())
    }
}

impl From<ToStrError> for Error {
    fn from(err: ToStrError) -> Self {
        Error::Header(err.to_string())
    }
}
