use crate::domain::{HttpAuth, HttpConfig, HttpHeader};
use crate::storage::Backend;
use serde::{Deserialize, Serialize};
use std::env;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;

pub const CURRENT_CONFIG_SCHEMA: u32 = 1;

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

impl FromStr for FileConflictStrategy {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "overwrite" => Ok(Self::Overwrite),
            "skipifvalid" | "skip_if_valid" | "skip-if-valid" => Ok(Self::SkipIfValid),
            "resume" => Ok(Self::Resume),
            other => Err(ConfigError::InvalidEnvValue {
                key: "PARADOWN_FILE_CONFLICT_STRATEGY".into(),
                value: other.to_string(),
                message: "expected overwrite, skip_if_valid, or resume".into(),
            }),
        }
    }
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
            threshold_bytes: 1024 * 1024,
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
    #[serde(default = "default_download_dir")]
    pub download_dir: PathBuf,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default = "default_concurrent_tasks", alias = "max_concurrent_downloads")]
    pub concurrent_tasks: usize,
    #[serde(default = "default_segments_per_task", alias = "worker_threads")]
    pub segments_per_task: usize,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub rate_limit_kbps: Option<NonZeroU64>,
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout: Duration,
    #[serde(default = "default_storage_backend", alias = "persistence_type")]
    pub storage_backend: Backend,
    #[serde(default)]
    pub progress_throttle: ProgressThrottleConfig,
    #[serde(default = "default_file_conflict_strategy")]
    pub file_conflict_strategy: FileConflictStrategy,
    #[serde(default = "default_debug")]
    pub debug: bool,
    #[serde(default)]
    pub http: HttpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default = "default_schema_version", alias = "version")]
    schema_version: u32,
    #[serde(flatten)]
    config: Config,
}

/// Builder 模式的实现
#[derive(Debug, Clone)]
pub struct ConfigBuilder {
    inner: Config,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            shuffle: false,
            concurrent_tasks: default_concurrent_tasks(),
            segments_per_task: default_segments_per_task(),
            retry: RetryConfig::default(),
            rate_limit_kbps: None,
            connection_timeout: default_connection_timeout(),
            storage_backend: default_storage_backend(),
            progress_throttle: ProgressThrottleConfig::default(),
            file_conflict_strategy: default_file_conflict_strategy(),
            debug: default_debug(),
            http: HttpConfig::default(),
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

    pub fn http(mut self, http: HttpConfig) -> Self {
        self.inner.http = http;
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
    #[error("Unsupported config schema version {found}, current supported version is {supported}")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    #[error("Invalid env value for {key}: '{value}' ({message})")]
    InvalidEnvValue {
        key: String,
        value: String,
        message: String,
    },
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
        let file: ConfigFile = toml::from_str(&content)?;
        if file.schema_version > CURRENT_CONFIG_SCHEMA {
            return Err(Box::new(ConfigError::UnsupportedSchemaVersion {
                found: file.schema_version,
                supported: CURRENT_CONFIG_SCHEMA,
            }));
        }

        Ok(file.config)
    }

    pub fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        if let Some(value) = read_env("PARADOWN_DOWNLOAD_DIR") {
            self.download_dir = PathBuf::from(value);
        }
        if let Some(value) = parse_env_bool("PARADOWN_SHUFFLE")? {
            self.shuffle = value;
        }
        if let Some(value) = parse_env_usize("PARADOWN_CONCURRENT_TASKS")? {
            self.concurrent_tasks = value;
        }
        if let Some(value) = parse_env_usize("PARADOWN_MAX_CONCURRENT")? {
            self.concurrent_tasks = value;
        }
        if let Some(value) = parse_env_usize("PARADOWN_SEGMENTS_PER_TASK")? {
            self.segments_per_task = value;
        }
        if let Some(value) = parse_env_usize("PARADOWN_WORKERS")? {
            self.segments_per_task = value;
        }
        if let Some(value) = parse_env_u64("PARADOWN_RATE_LIMIT_KBPS")? {
            self.rate_limit_kbps = NonZeroU64::new(value);
        }
        if let Some(value) = parse_env_u64("PARADOWN_CONNECTION_TIMEOUT_SECS")? {
            self.connection_timeout = Duration::from_secs(value);
        }
        if let Some(value) = read_env("PARADOWN_STORAGE_BACKEND") {
            self.storage_backend = parse_storage_backend_env(&value)?;
        }
        if let Some(value) = read_env("PARADOWN_FILE_CONFLICT_STRATEGY") {
            self.file_conflict_strategy = FileConflictStrategy::from_str(&value)?;
        }
        if let Some(value) = parse_env_bool("PARADOWN_DEBUG")? {
            self.debug = value;
        }

        if let Some(value) = parse_env_bool("PARADOWN_USE_ENV_PROXY")? {
            self.http.client.proxy.use_env_proxy = value;
        }
        if let Some(value) = read_env("PARADOWN_HTTP_PROXY") {
            self.http.client.proxy.http_proxy = Some(value);
        }
        if let Some(value) = read_env("PARADOWN_HTTPS_PROXY") {
            self.http.client.proxy.https_proxy = Some(value);
        }
        if let Some(value) = read_env("PARADOWN_NO_PROXY") {
            self.http.client.proxy.no_proxy = Some(value);
        }

        if let Some(value) = read_env("PARADOWN_HEADERS") {
            self.http.request.headers = parse_headers_env("PARADOWN_HEADERS", &value)?;
        }
        if let Some(value) = read_env("PARADOWN_COOKIE") {
            self.http.request.cookie = Some(value);
        }
        if let Some(value) = read_env("PARADOWN_USER_AGENT") {
            self.http.request.user_agent = Some(value);
        }
        if let Some(value) = read_env("PARADOWN_BASIC_AUTH") {
            self.http.request.auth = Some(parse_basic_auth_env(&value)?);
        }
        if let Some(value) = read_env("PARADOWN_BEARER_TOKEN") {
            self.http.request.auth = Some(HttpAuth::Bearer { token: value });
        }

        Ok(())
    }
}

impl FromStr for Config {
    type Err = toml::de::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let file: ConfigFile = toml::from_str(s)?;
        Ok(file.config)
    }
}

fn default_schema_version() -> u32 {
    CURRENT_CONFIG_SCHEMA
}

fn default_download_dir() -> PathBuf {
    PathBuf::from("./downloads")
}

fn default_concurrent_tasks() -> usize {
    4
}

fn default_segments_per_task() -> usize {
    4
}

fn default_connection_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_storage_backend() -> Backend {
    Backend::Sqlite("./downloads.db".into())
}

fn default_file_conflict_strategy() -> FileConflictStrategy {
    FileConflictStrategy::Resume
}

fn default_debug() -> bool {
    true
}

fn read_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_env_bool(key: &str) -> Result<Option<bool>, ConfigError> {
    let Some(value) = read_env(key) else {
        return Ok(None);
    };

    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(Some(true)),
        "0" | "false" | "no" | "off" => Ok(Some(false)),
        _ => Err(ConfigError::InvalidEnvValue {
            key: key.to_string(),
            value,
            message: "expected boolean".into(),
        }),
    }
}

fn parse_env_usize(key: &str) -> Result<Option<usize>, ConfigError> {
    let Some(value) = read_env(key) else {
        return Ok(None);
    };

    value
        .parse::<usize>()
        .map(Some)
        .map_err(|_| ConfigError::InvalidEnvValue {
            key: key.to_string(),
            value,
            message: "expected unsigned integer".into(),
        })
}

fn parse_env_u64(key: &str) -> Result<Option<u64>, ConfigError> {
    let Some(value) = read_env(key) else {
        return Ok(None);
    };

    value
        .parse::<u64>()
        .map(Some)
        .map_err(|_| ConfigError::InvalidEnvValue {
            key: key.to_string(),
            value,
            message: "expected unsigned integer".into(),
        })
}

fn parse_storage_backend_env(value: &str) -> Result<Backend, ConfigError> {
    let lowered = value.trim().to_ascii_lowercase();
    if lowered == "memory" {
        return Ok(Backend::Memory);
    }
    if let Some(path) = value.strip_prefix("sqlite:") {
        return Ok(Backend::Sqlite(PathBuf::from(path)));
    }
    if let Some(path) = value.strip_prefix("json:") {
        return Ok(Backend::JsonFile(path.to_string()));
    }

    Err(ConfigError::InvalidEnvValue {
        key: "PARADOWN_STORAGE_BACKEND".into(),
        value: value.to_string(),
        message: "expected memory, sqlite:/path/to/db, or json:/path/to/file".into(),
    })
}

fn parse_headers_env(key: &str, value: &str) -> Result<Vec<HttpHeader>, ConfigError> {
    let mut headers = Vec::new();

    for raw_line in value.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let Some((name, header_value)) = line.split_once(':') else {
            return Err(ConfigError::InvalidEnvValue {
                key: key.to_string(),
                value: line.to_string(),
                message: "expected one header per line in the form 'Name: Value'".into(),
            });
        };

        headers.push(HttpHeader {
            name: name.trim().to_string(),
            value: header_value.trim().to_string(),
        });
    }

    Ok(headers)
}

fn parse_basic_auth_env(value: &str) -> Result<HttpAuth, ConfigError> {
    let (username, password) = match value.split_once(':') {
        Some((username, password)) => (
            username.trim().to_string(),
            Some(password.trim().to_string()).filter(|value| !value.is_empty()),
        ),
        None => (value.trim().to_string(), None),
    };

    if username.is_empty() {
        return Err(ConfigError::InvalidEnvValue {
            key: "PARADOWN_BASIC_AUTH".into(),
            value: value.to_string(),
            message: "expected username[:password]".into(),
        });
    }

    Ok(HttpAuth::Basic { username, password })
}

#[cfg(test)]
mod tests {
    use super::{CURRENT_CONFIG_SCHEMA, Config, FileConflictStrategy};

    #[test]
    fn parses_legacy_field_aliases() {
        let config = r#"
            worker_threads = 8
            max_concurrent_downloads = 6
            persistence_type = { Memory = {} }
        "#;

        let parsed = config.parse::<Config>().unwrap();
        assert_eq!(parsed.segments_per_task, 8);
        assert_eq!(parsed.concurrent_tasks, 6);
        assert!(matches!(parsed.storage_backend, crate::storage::Backend::Memory));
    }

    #[test]
    fn parses_schema_wrapped_config() {
        let config = format!(
            r#"
            schema_version = {CURRENT_CONFIG_SCHEMA}
            download_dir = "./custom-downloads"
            file_conflict_strategy = "Overwrite"
            "#
        );

        let parsed = config.parse::<Config>().unwrap();
        assert_eq!(parsed.download_dir, std::path::PathBuf::from("./custom-downloads"));
        assert!(matches!(
            parsed.file_conflict_strategy,
            FileConflictStrategy::Overwrite
        ));
    }
}
