use crate::persistence::PersistenceType;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;

/// 文件存在处理策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileConflictStrategy {
    /// 删除已存在的文件并重新下载
    Overwrite,
    /// 如果文件完整则跳过下载
    SkipIfValid,
    /// 保留已存在文件（断点续传）
    Resume,
}

/// 进度节流配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressThrottleConfig {
    /// 事件最小触发间隔(ms)
    pub interval_ms: u64,
    /// 进度变化最小阈值(bytes)
    pub threshold_bytes: u64,
}

impl Default for ProgressThrottleConfig {
    fn default() -> Self {
        Self {
            interval_ms: 200,
            threshold_bytes: 1024 * 1024, // 1MB
        }
    }
}

/// 重试机制配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_delay: u64,
    pub max_delay: u64,
    pub backoff_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: 1,
            max_delay: 30,
            backoff_factor: 2.0,
        }
    }
}

/// 下载配置主结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    pub download_dir: PathBuf,
    pub urls: Vec<String>,
    pub shuffle: bool,
    pub max_concurrent_downloads: usize,
    pub worker_threads: usize,
    pub retry: RetryConfig,
    pub rate_limit_kbps: Option<NonZeroU64>,
    pub connection_timeout: Duration,
    pub persistence_type: PersistenceType,
    pub progress_throttle: ProgressThrottleConfig,
    pub file_conflict_strategy: FileConflictStrategy,
    pub debug: bool,
}

/// Builder 模式的实现
#[derive(Debug, Clone)]
pub struct DownloadConfigBuilder {
    inner: DownloadConfig,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            download_dir: PathBuf::from("downloads"),
            urls: Vec::new(),
            shuffle: false,
            max_concurrent_downloads: 4,
            worker_threads: 4,
            retry: RetryConfig::default(),
            rate_limit_kbps: None,
            connection_timeout: Duration::from_secs(30),
            persistence_type: PersistenceType::Sqlite("downloads.db".into()),
            progress_throttle: ProgressThrottleConfig::default(),
            file_conflict_strategy: FileConflictStrategy::Resume,
            debug: true,
        }
    }
}

impl DownloadConfigBuilder {
    /// 创建新的 Builder
    pub fn new() -> Self {
        Self {
            inner: DownloadConfig::default(),
        }
    }

    pub fn download_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.inner.download_dir = dir.into();
        self
    }

    pub fn urls(mut self, urls: Vec<String>) -> Self {
        self.inner.urls = urls;
        self
    }

    pub fn add_url(mut self, url: impl Into<String>) -> Self {
        self.inner.urls.push(url.into());
        self
    }

    pub fn shuffle(mut self, enable: bool) -> Self {
        self.inner.shuffle = enable;
        self
    }

    pub fn max_concurrent_downloads(mut self, n: usize) -> Self {
        self.inner.max_concurrent_downloads = n;
        self
    }

    pub fn worker_threads(mut self, n: usize) -> Self {
        self.inner.worker_threads = n;
        self
    }

    pub fn retry(mut self, retry: RetryConfig) -> Self {
        self.inner.retry = retry;
        self
    }

    pub fn rate_limit_kbps(mut self, kbps: Option<NonZeroU64>) -> Self {
        self.inner.rate_limit_kbps = kbps;
        self
    }

    pub fn connection_timeout(mut self, secs: u64) -> Self {
        self.inner.connection_timeout = Duration::from_secs(secs);
        self
    }

    pub fn persistence_type(mut self, p: PersistenceType) -> Self {
        self.inner.persistence_type = p;
        self
    }

    pub fn progress_throttle(mut self, cfg: ProgressThrottleConfig) -> Self {
        self.inner.progress_throttle = cfg;
        self
    }

    pub fn file_conflict_strategy(mut self, strategy: FileConflictStrategy) -> Self {
        self.inner.file_conflict_strategy = strategy;
        self
    }

    pub fn debug(mut self, debug: bool) -> Self {
        self.inner.debug = debug;
        self
    }

    /// 构建配置并验证
    pub fn build(self) -> Result<DownloadConfig, DownloadConfigError> {
        self.inner.validate()?;
        Ok(self.inner)
    }
}

#[derive(Debug, Error)]
pub enum DownloadConfigError {
    #[error("Invalid download directory: {0}")]
    InvalidDownloadDir(String),
    #[error("Invalid number of workers: {0}")]
    InvalidWorkers(usize),
    #[error("No download URLs provided")]
    NoUrls,
    #[error("Invalid URL format: {0}")]
    InvalidUrl(String),
}

impl DownloadConfig {
    /// 验证配置合法性
    pub fn validate(&self) -> Result<(), DownloadConfigError> {
        if !self.download_dir.to_str().map_or(false, |s| !s.is_empty()) {
            return Err(DownloadConfigError::InvalidDownloadDir(
                self.download_dir.to_string_lossy().to_string(),
            ));
        }

        if let Err(e) = std::fs::create_dir_all(&self.download_dir) {
            return Err(DownloadConfigError::InvalidDownloadDir(format!(
                "Cannot create directory '{}': {}",
                self.download_dir.display(),
                e
            )));
        }

        if self.worker_threads == 0 || self.worker_threads > 100 {
            return Err(DownloadConfigError::InvalidWorkers(self.worker_threads));
        }

        if self.urls.is_empty() {
            return Err(DownloadConfigError::NoUrls);
        }

        if self.urls.len() > 163 {
            return Err(DownloadConfigError::InvalidUrl(format!(
                "Too many URLs (max 163, got {})",
                self.urls.len()
            )));
        }

        for (i, url) in self.urls.iter().enumerate() {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(DownloadConfigError::InvalidUrl(format!(
                    "Invalid URL at index {}: {}",
                    i, url
                )));
            }
        }

        Ok(())
    }

    /// 从 TOML 文件加载配置
    pub fn from_file(path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
}

impl FromStr for DownloadConfig {
    type Err = toml::de::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        toml::from_str(s)
    }
}
