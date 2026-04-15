use crate::storage::Backend;
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
pub struct Config {
    pub download_dir: PathBuf,
    pub shuffle: bool,
    pub concurrent_tasks: usize,
    pub segments_per_task: usize,
    pub retry: RetryConfig,
    pub rate_limit_kbps: Option<NonZeroU64>,
    pub connection_timeout: Duration,
    pub storage_backend: Backend,
    pub progress_throttle: ProgressThrottleConfig,
    pub file_conflict_strategy: FileConflictStrategy,
    pub debug: bool,
}

/// Builder 模式的实现
#[derive(Debug, Clone)]
pub struct ConfigBuilder {
    inner: Config,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            download_dir: PathBuf::from("./downloads"),
            shuffle: false,
            concurrent_tasks: 4,
            segments_per_task: 4,
            retry: RetryConfig::default(),
            rate_limit_kbps: None,
            connection_timeout: Duration::from_secs(30),
            storage_backend: Backend::Sqlite("./downloads.db".into()),
            progress_throttle: ProgressThrottleConfig::default(),
            file_conflict_strategy: FileConflictStrategy::Resume,
            debug: true,
        }
    }
}

impl ConfigBuilder {
    /// 创建新的 Builder
    pub fn new() -> Self {
        Self {
            inner: Config::default(),
        }
    }

    pub fn download_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.inner.download_dir = dir.into();
        self
    }

    pub fn shuffle(mut self, enable: bool) -> Self {
        self.inner.shuffle = enable;
        self
    }

    pub fn concurrent_tasks(mut self, n: usize) -> Self {
        self.inner.concurrent_tasks = n;
        self
    }

    pub fn segments_per_task(mut self, n: usize) -> Self {
        self.inner.segments_per_task = n;
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

    pub fn storage_backend(mut self, backend: Backend) -> Self {
        self.inner.storage_backend = backend;
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
    pub fn build(self) -> Result<Config, ConfigError> {
        self.inner.validate()?;
        Ok(self.inner)
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Invalid download directory: {0}")]
    InvalidDownloadDir(String),
    #[error("Invalid segments per task: {0}")]
    InvalidSegmentsPerTask(usize),
    #[error("No download URLs provided")]
    NoUrls,
    #[error("Invalid URL format: {0}")]
    InvalidUrl(String),
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.download_dir.exists()
            && let Err(e) = std::fs::create_dir_all(&self.download_dir)
        {
            return Err(ConfigError::InvalidDownloadDir(format!(
                "Cannot create directory '{}': {}",
                self.download_dir.display(),
                e
            )));
        }

        if self.segments_per_task == 0 || self.segments_per_task > 100 {
            return Err(ConfigError::InvalidSegmentsPerTask(self.segments_per_task));
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

impl FromStr for Config {
    type Err = toml::de::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        toml::from_str(s)
    }
}
