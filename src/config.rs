use crate::domain::{HttpAuth, HttpConfig, HttpHeader};
use crate::storage::Backend;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use std::env;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::str::FromStr;
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

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

impl LogLevel {
    pub fn as_level_filter(self) -> LevelFilter {
        match self {
            Self::Error => LevelFilter::Error,
            Self::Warn => LevelFilter::Warn,
            Self::Info => LevelFilter::Info,
            Self::Debug => LevelFilter::Debug,
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
    #[serde(default, alias = "shuffle")]
    pub shuffle_tasks: bool,
    #[serde(
        default = "default_concurrent_tasks",
        alias = "max_concurrent_downloads"
    )]
    pub concurrent_tasks: usize,
    #[serde(default = "default_segments_per_task", alias = "worker_threads")]
    pub segments_per_task: usize,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default, alias = "rate_limit_kbps")]
    pub rate_limit_kib_per_sec: Option<NonZeroU64>,
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_storage_backend", alias = "persistence_type")]
    pub storage_backend: Backend,
    #[serde(default)]
    pub progress_throttle: ProgressThrottleConfig,
    #[serde(default = "default_file_conflict_strategy")]
    pub file_conflict_strategy: FileConflictStrategy,
    #[serde(default)]
    pub log_level: LogLevel,
    #[serde(default, alias = "on_complete")]
    pub completion_hook: Option<String>,
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
            shuffle_tasks: false,
            concurrent_tasks: default_concurrent_tasks(),
            segments_per_task: default_segments_per_task(),
            retry: RetryConfig::default(),
            rate_limit_kib_per_sec: None,
            connect_timeout_secs: default_connect_timeout_secs(),
            storage_backend: default_storage_backend(),
            progress_throttle: ProgressThrottleConfig::default(),
            file_conflict_strategy: default_file_conflict_strategy(),
            log_level: LogLevel::default(),
            completion_hook: None,
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

    pub fn shuffle_tasks(mut self, enable: bool) -> Self {
        self.inner.shuffle_tasks = enable;
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

    pub fn rate_limit_kib_per_sec(mut self, kib_per_sec: Option<NonZeroU64>) -> Self {
        self.inner.rate_limit_kib_per_sec = kib_per_sec;
        self
    }

    pub fn connect_timeout_secs(mut self, secs: u64) -> Self {
        self.inner.connect_timeout_secs = secs;
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

    pub fn log_level(mut self, log_level: LogLevel) -> Self {
        self.inner.log_level = log_level;
        self
    }

    pub fn completion_hook(mut self, command: impl Into<String>) -> Self {
        self.inner.completion_hook = Some(command.into());
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
    #[error("Invalid concurrent tasks: {0}")]
    InvalidConcurrentTasks(usize),
    #[error("Invalid segments per task: {0}")]
    InvalidSegmentsPerTask(usize),
    #[error("Invalid progress throttle interval: {0}")]
    InvalidProgressThrottleInterval(u64),
    #[error("Invalid retry config: {0}")]
    InvalidRetryConfig(String),
    #[error("Completion hook cannot be blank")]
    InvalidCompletionHook,
    #[error("Unsupported config schema version {found}, current supported version is {supported}")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    #[error("Invalid env value for {key}: '{value}' ({message})")]
    InvalidEnvValue {
        key: String,
        value: String,
        message: String,
    },
}

#[derive(Debug, Error)]
pub enum ConfigLoadError {
    #[error("Failed to read config file '{path}': {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Failed to parse config file '{path}': {source}")]
    ParseFile {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("Failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error(transparent)]
    Config(#[from] ConfigError),
}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.download_dir.as_os_str().is_empty() {
            return Err(ConfigError::InvalidDownloadDir(
                "path cannot be empty".into(),
            ));
        }

        if self.concurrent_tasks == 0 {
            return Err(ConfigError::InvalidConcurrentTasks(self.concurrent_tasks));
        }

        if self.segments_per_task == 0 || self.segments_per_task > 100 {
            return Err(ConfigError::InvalidSegmentsPerTask(self.segments_per_task));
        }

        if self.progress_throttle.interval_ms == 0 {
            return Err(ConfigError::InvalidProgressThrottleInterval(
                self.progress_throttle.interval_ms,
            ));
        }

        validate_retry_config(&self.retry)?;

        if self
            .completion_hook
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(ConfigError::InvalidCompletionHook);
        }

        Ok(())
    }

    /// 从 TOML 文件加载配置
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigLoadError> {
        let path = path.as_ref();
        let content =
            std::fs::read_to_string(path).map_err(|source| ConfigLoadError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;
        Self::parse_toml_document(&content)
            .map_err(|source| ConfigLoadError::ParseFile {
                path: path.into(),
                source,
            })?
            .into_config()
    }

    pub fn from_toml_str(content: &str) -> Result<Self, ConfigLoadError> {
        Self::parse_toml_document(content)?.into_config()
    }

    pub fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        if let Some(value) = read_env("PARADOWN_DOWNLOAD_DIR") {
            self.download_dir = PathBuf::from(value);
        }
        if let Some(value) = parse_env_bool("PARADOWN_SHUFFLE_TASKS")? {
            self.shuffle_tasks = value;
        }
        if let Some(value) = parse_env_bool("PARADOWN_SHUFFLE")? {
            self.shuffle_tasks = value;
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
        if let Some(value) = parse_env_u64("PARADOWN_RATE_LIMIT_KIB_PER_SEC")? {
            self.rate_limit_kib_per_sec = NonZeroU64::new(value);
        }
        if let Some(value) = parse_env_u64("PARADOWN_RATE_LIMIT_KBPS")? {
            self.rate_limit_kib_per_sec = NonZeroU64::new(value);
        }
        if let Some(value) = parse_env_u64("PARADOWN_CONNECT_TIMEOUT_SECS")? {
            self.connect_timeout_secs = value;
        }
        if let Some(value) = parse_env_u64("PARADOWN_CONNECTION_TIMEOUT_SECS")? {
            self.connect_timeout_secs = value;
        }
        if let Some(value) = read_env("PARADOWN_STORAGE_BACKEND") {
            self.storage_backend = parse_storage_backend_env(&value)?;
        }
        if let Some(value) = read_env("PARADOWN_FILE_CONFLICT_STRATEGY") {
            self.file_conflict_strategy = FileConflictStrategy::from_str(&value)?;
        }
        if let Some(value) = read_env("PARADOWN_LOG_LEVEL") {
            self.log_level = parse_log_level("PARADOWN_LOG_LEVEL", &value)?;
        }
        if let Some(value) = read_env("PARADOWN_COMPLETION_HOOK") {
            self.completion_hook = Some(value);
        }
        if let Some(value) = read_env("PARADOWN_ON_COMPLETE") {
            self.completion_hook = Some(value);
        }

        if let Some(value) = parse_env_bool("PARADOWN_USE_ENV_PROXY")? {
            self.http.client.proxy.use_env_proxy = value;
        }
        if let Some(value) = parse_env_bool("PARADOWN_COOKIE_STORE")? {
            self.http.client.cookie_store = value;
        }
        if let Some(value) = read_env("PARADOWN_COOKIE_JAR") {
            self.http.client.cookie_store = true;
            self.http.client.cookie_jar_path = Some(value.into());
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
        if let Some(value) = parse_env_bool("PARADOWN_TLS_INSECURE_SKIP_VERIFY")? {
            self.http.client.tls.insecure_skip_verify = value;
        }
        if let Some(value) = read_env("PARADOWN_TLS_CA_CERT_PEM") {
            self.http.client.tls.ca_certificate_pem = Some(value.into());
        }
        if let Some(value) = read_env("PARADOWN_TLS_CLIENT_IDENTITY_PEM") {
            self.http.client.tls.client_identity_pem = Some(value.into());
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

    fn parse_toml_document(content: &str) -> Result<ConfigFile, toml::de::Error> {
        toml::from_str(content)
    }
}

impl FromStr for Config {
    type Err = ConfigLoadError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_toml_str(s)
    }
}

impl ConfigFile {
    fn into_config(self) -> Result<Config, ConfigLoadError> {
        validate_schema_version(self.schema_version)?;
        self.config.validate()?;
        Ok(self.config)
    }
}

fn validate_schema_version(schema_version: u32) -> Result<(), ConfigError> {
    if schema_version > CURRENT_CONFIG_SCHEMA {
        return Err(ConfigError::UnsupportedSchemaVersion {
            found: schema_version,
            supported: CURRENT_CONFIG_SCHEMA,
        });
    }

    Ok(())
}

fn validate_retry_config(retry: &RetryConfig) -> Result<(), ConfigError> {
    if retry.max_retries > 0 && retry.initial_delay == 0 {
        return Err(ConfigError::InvalidRetryConfig(
            "initial_delay must be greater than 0 when retries are enabled".into(),
        ));
    }

    if retry.max_delay < retry.initial_delay {
        return Err(ConfigError::InvalidRetryConfig(
            "max_delay must be greater than or equal to initial_delay".into(),
        ));
    }

    if retry.backoff_factor < 1.0 {
        return Err(ConfigError::InvalidRetryConfig(
            "backoff_factor must be greater than or equal to 1.0".into(),
        ));
    }

    Ok(())
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

fn default_connect_timeout_secs() -> u64 {
    30
}

fn default_storage_backend() -> Backend {
    Backend::Sqlite("./downloads.db".into())
}

fn default_file_conflict_strategy() -> FileConflictStrategy {
    FileConflictStrategy::Resume
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

fn parse_log_level(key: &str, value: &str) -> Result<LogLevel, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "error" => Ok(LogLevel::Error),
        "warn" | "warning" => Ok(LogLevel::Warn),
        "info" => Ok(LogLevel::Info),
        "debug" => Ok(LogLevel::Debug),
        _ => Err(ConfigError::InvalidEnvValue {
            key: key.to_string(),
            value: value.to_string(),
            message: "expected error, warn, info, or debug".into(),
        }),
    }
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
    use super::{
        CURRENT_CONFIG_SCHEMA, Config, ConfigBuilder, ConfigLoadError, FileConflictStrategy,
        LogLevel,
    };
    use tempfile::tempdir;

    #[test]
    fn parses_legacy_field_aliases() {
        let config = r#"
            shuffle = true
            worker_threads = 8
            max_concurrent_downloads = 6
            rate_limit_kbps = 512
            persistence_type = { Memory = {} }
            on_complete = "echo done"
        "#;

        let parsed = config.parse::<Config>().unwrap();
        assert!(parsed.shuffle_tasks);
        assert_eq!(parsed.segments_per_task, 8);
        assert_eq!(parsed.concurrent_tasks, 6);
        assert_eq!(parsed.rate_limit_kib_per_sec.unwrap().get(), 512);
        assert_eq!(parsed.completion_hook.as_deref(), Some("echo done"));
        assert!(matches!(
            parsed.storage_backend,
            crate::storage::Backend::Memory
        ));
    }

    #[test]
    fn parses_schema_wrapped_config() {
        let config = format!(
            r#"
            schema_version = {CURRENT_CONFIG_SCHEMA}
            download_dir = "./custom-downloads"
            shuffle_tasks = true
            rate_limit_kib_per_sec = 1024
            connect_timeout_secs = 45
            file_conflict_strategy = "Overwrite"
            log_level = "debug"
            completion_hook = "echo finished"
            "#
        );

        let parsed = config.parse::<Config>().unwrap();
        assert_eq!(
            parsed.download_dir,
            std::path::PathBuf::from("./custom-downloads")
        );
        assert!(parsed.shuffle_tasks);
        assert_eq!(parsed.rate_limit_kib_per_sec.unwrap().get(), 1024);
        assert_eq!(parsed.connect_timeout_secs, 45);
        assert_eq!(parsed.log_level, LogLevel::Debug);
        assert_eq!(parsed.completion_hook.as_deref(), Some("echo finished"));
        assert!(matches!(
            parsed.file_conflict_strategy,
            FileConflictStrategy::Overwrite
        ));
    }

    #[test]
    fn from_str_rejects_unsupported_schema_version() {
        let config = format!("schema_version = {}", CURRENT_CONFIG_SCHEMA + 1);
        let err = config.parse::<Config>().unwrap_err();

        assert!(matches!(
            err,
            ConfigLoadError::Config(super::ConfigError::UnsupportedSchemaVersion { .. })
        ));
    }

    #[test]
    fn validate_is_pure_and_does_not_create_directories() {
        let temp = tempdir().unwrap();
        let download_dir = temp.path().join("not-created-yet");
        let config = ConfigBuilder::new()
            .download_dir(download_dir.clone())
            .build()
            .unwrap();

        assert!(!download_dir.exists());
        config.validate().unwrap();
        assert!(!download_dir.exists());
    }
}
