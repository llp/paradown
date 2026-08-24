// =================================================================================
// 1. 模块导入与依赖引用 (use Statements)
// =================================================================================
use crate::domain::{HttpAuth, HttpConfig, HttpHeader};
use crate::p2p::{LibtorrentEngineConfig, SwarmIndexProviderConfig, SwarmProviderConfig};
use crate::storage::Backend;
use log::LevelFilter;
// `serde::{Deserialize, Serialize}`：Serde 库的核心 trait，用于数据的序列化（结构体转 TOML/JSON）与反序列化（TOML/JSON 转结构体）
use serde::{Deserialize, Serialize};
use std::env;
// `NonZeroU64`：标准库提供的非零无符号 64 位整数类型。
// Rust 的空指针/利基优化（Niche Optimization）：`Option<NonZeroU64>` 在内存中占用的大小与普通的 `u64` 完全相同（用 0 字节表示 None），消除了内存浪费！
use std::num::NonZeroU64;

// `Path` 和 `PathBuf` 用于处理文件系统路径，它们的关系类似于 `str` 和 `String`：
// - `Path`: 路径切片引用（不可变、无所有权、动态大小类型 DST），通常以 `&Path` 形式传递。
// - `PathBuf`: 拥有所有权的路径缓冲区（在堆上分配），可变，类似于 `String`。
use std::path::{Path, PathBuf};
use std::str::FromStr;
// `thiserror::Error`：第三方库 thiserror 提供的派生宏，帮助极其简便地定义符合 `std::error::Error` 标准接口的自定义错误类型
use thiserror::Error;

/// 全局配置 Schema 版本号常量
pub const CURRENT_CONFIG_SCHEMA: u32 = 1;

// =================================================================================
// 2. 枚举类型与转换 Trait (Enums & Traits)
// =================================================================================

/// 文件存在时的冲突处理策略枚举
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileConflictStrategy {
    /// 覆盖：删除已存在的文件并重新下载
    Overwrite,
    /// 校验跳过：如果本地文件已存在且校验通过，跳过下载
    SkipIfValid,
    /// 续传：保留已存在的文件进度（断点续传）
    Resume,
}

// 为 `FileConflictStrategy` 实现标准库 `FromStr` Trait。
// 允许使用 `"overwrite".parse::<FileConflictStrategy>()` 或字符串转枚举
impl FromStr for FileConflictStrategy {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // `.trim()` 去除首尾空格，`.to_ascii_lowercase()` 转小写，`.as_str()` 借用为 &str 进 match 模式匹配
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

/// 日志输出级别枚举
/// - `#[serde(rename_all = "snake_case")]`: serde 属性宏，序列化/反序列化时自动转为蛇形小写（如 "snake_case"）
/// - `#[default]`: Rust 1.62+ 提供的语法，标记 `Info` 为派生 `Default` 特质时的默认变体
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
    /// 将自定义 LogLevel 映射转换为 log 库的 `LevelFilter` 过滤器
    pub fn as_level_filter(self) -> LevelFilter {
        match self {
            Self::Error => LevelFilter::Error,
            Self::Warn => LevelFilter::Warn,
            Self::Info => LevelFilter::Info,
            Self::Debug => LevelFilter::Debug,
        }
    }
}

// =================================================================================
// 3. 配置子结构体定义 (Sub-Configs)
// =================================================================================

/// 进度更新节流阀配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressThrottleConfig {
    /// 事件最小触发间隔 (毫秒 ms)
    pub interval_ms: u64,
    /// 进度变化最小触发阈值 (字节 bytes)
    pub threshold_bytes: u64,
}

// 手动实现 `Default` Trait 赋予默认推荐值
impl Default for ProgressThrottleConfig {
    fn default() -> Self {
        Self {
            interval_ms: 200,
            threshold_bytes: 1024 * 1024, // 默认 1 MB 触发一次
        }
    }
}

/// 失败重试策略配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_delay: u64,
    pub max_delay: u64,
    pub backoff_factor: f64, // 退避乘积因子（如 2.0 代表指数退避 Exponential Backoff）
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

/// P2P / Swarm 下载扩展配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2pConfig {
    // `#[serde(default = "default_p2p_enabled")]`: serde 属性宏，若配置文件缺少该字段，自动调用函数指定默认值
    #[serde(default = "default_p2p_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub libtorrent: LibtorrentEngineConfig,
    #[serde(default)]
    pub swarm: SwarmProviderConfig,
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            enabled: default_p2p_enabled(),
            libtorrent: LibtorrentEngineConfig::default(),
            swarm: SwarmProviderConfig::default(),
        }
    }
}

// =================================================================================
// 4. 下载引擎主配置结构体 (Main Config)
// =================================================================================

/// 下载引擎的主配置结构体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 本地文件保存下载目录
    #[serde(default = "default_download_dir")]
    pub download_dir: PathBuf,

    /// 是否对任务打乱随机排序
    #[serde(default, alias = "shuffle")]
    pub shuffle_tasks: bool,

    /// 最大并发任务数上限 (Semaphore 控制)
    #[serde(
        default = "default_concurrent_tasks",
        alias = "max_concurrent_downloads"
    )]
    pub concurrent_tasks: usize,

    /// 单个 HTTP 任务最大的 Worker 分片并发线程数
    #[serde(default = "default_segments_per_task", alias = "worker_threads")]
    pub segments_per_task: usize,

    /// 重试策略
    #[serde(default)]
    pub retry: RetryConfig,

    /// 全局限速 (KiB/s)，使用 `Option<NonZeroU64>` 既节省空间又防止配置为 0
    #[serde(default, alias = "rate_limit_kbps")]
    pub rate_limit_kib_per_sec: Option<NonZeroU64>,

    /// 网络连接超时时间 (秒)
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,

    /// 存储后端类型 (如 SQLite)
    #[serde(default = "default_storage_backend", alias = "persistence_type")]
    pub storage_backend: Backend,

    /// 进度更新节流配置
    #[serde(default)]
    pub progress_throttle: ProgressThrottleConfig,

    /// 文件冲突处理策略 (Overwrite, SkipIfValid, Resume)
    #[serde(default = "default_file_conflict_strategy")]
    pub file_conflict_strategy: FileConflictStrategy,

    /// 全局日志级别
    #[serde(default)]
    pub log_level: LogLevel,

    /// 下载完成后的可执行 Shell 钩子脚本命令
    #[serde(default, alias = "on_complete")]
    pub completion_hook: Option<String>,

    /// HTTP 代理与网络请求配置
    #[serde(default)]
    pub http: HttpConfig,

    /// P2P 引擎配置
    #[serde(default)]
    pub p2p: P2pConfig,
}

/// 内部 TOML 文件包装结构体，包含 Schema 版本号
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default = "default_schema_version", alias = "version")]
    schema_version: u32,
    // `#[serde(flatten)]`: serde 扁平化属性宏，将 `Config` 的所有字段直接平铺到 TOML 顶层
    #[serde(flatten)]
    config: Config,
}

// =================================================================================
// 5. Builder 构建器模式实现 (ConfigBuilder)
// =================================================================================

/// `ConfigBuilder` 结构体：实现流畅调用 (Fluent API) 的 Builder 设计模式
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
            p2p: P2pConfig::default(),
        }
    }
}

impl ConfigBuilder {
    /// 创建新的 ConfigBuilder 实例
    pub fn new() -> Self {
        Self {
            inner: Config::default(),
        }
    }

    // `mut self` 与 `impl Into<PathBuf>` 语法解析：
    // - `mut self`: 接收 `self` 的所有权并将变量标记为可变（Ownership Transfer）。修改后返回 `Self` 自身，实现链式流畅调用。
    // - `impl Into<PathBuf>`: 泛型特质约束（Trait Bound）。调用者可以传入 `&str`、`String` 或 `PathBuf`，内部通过 `dir.into()` 自动转换为 `PathBuf`！
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

    pub fn p2p(mut self, p2p: P2pConfig) -> Self {
        self.inner.p2p = p2p;
        self
    }

    /// 构建 `Config` 实例并调用 `validate()` 进行合法性校验
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

// =================================================================================
// 6. 自定义错误枚举 (ConfigError & ConfigLoadError) 与 thiserror 库使用
// =================================================================================

/// 配置校验非法错误枚举
/// `#[derive(Debug, Error)]`: `thiserror` 库提供的派生宏，自动实现 `std::fmt::Display` 与 `std::error::Error`
#[derive(Debug, Error)]
pub enum ConfigError {
    // `#[error("...")]`: 指定该错误变体的 Display 格式化模板，`{0}` 表示元组变体的第 0 个字段
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

    #[error("Invalid p2p config: {0}")]
    InvalidP2pConfig(String),

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

/// 配置加载错误枚举 (文件 IO、TOML 解析、环境变量)
#[derive(Debug, Error)]
pub enum ConfigLoadError {
    #[error("Failed to read config file '{path}': {source}")]
    ReadFile {
        path: PathBuf,
        // `#[source]`: 标记内部嵌入的底层错误（使 Error::source() 可以递归获取根因错误）
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse config file '{path}': {source}")]
    ParseFile {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    // `#[from]`: 自动生成 `From<toml::de::Error> for ConfigLoadError`，使 `?` 运算符可自动将 toml 错误转换为 ConfigLoadError
    #[error("Failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),

    // `#[error(transparent)]`: 透明转发内部错误的 Display 和 source 方法
    #[error(transparent)]
    Config(#[from] ConfigError),
}

// =================================================================================
// 7. 配置合法性校验与加载实现 (Config Impl)
// =================================================================================

impl Config {
    /// 校验配置项参数合法性 (数值边界检查)
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
        validate_p2p_config(&self.p2p)?;

        // `.as_deref()`: 将 `Option<String>` 借用转换为 `Option<&str>`
        // `.is_some_and(|value| ...)`: 如果 Option 为 Some 且里面的闭包条件返回 true
        if self
            .completion_hook
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(ConfigError::InvalidCompletionHook);
        }

        Ok(())
    }

    /// 从 TOML 文件路径加载配置
    /// `path: impl AsRef<Path>`: 泛型借用约束，允许调用者传入 `&str`、`String`、`&Path` 或 `PathBuf`
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

    /// 从环境变量中读取值来覆盖当前的配置。
    ///
    /// 这个方法会修改 `Config` 实例自身，因为它接收 `&mut self` 参数。
    /// `&mut self` 表示一个对实例的可变引用，允许函数内部修改实例的字段。
    ///
    /// # 返回值
    ///
    /// - `Ok(())`: 如果所有环境变量都被成功解析并应用，则返回成功。
    ///   `()` 是单元类型，表示成功时不返回任何有意义的值。
    /// - `Err(ConfigError)`: 如果解析任何一个环境变量时出错（例如，值格式不正确），
    ///   则返回一个包含具体错误信息的 `ConfigError`。
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
        if let Some(value) = parse_env_bool("PARADOWN_P2P_ENABLED")? {
            self.p2p.enabled = value;
        }
        if let Some(value) = parse_env_usize("PARADOWN_LIBTORRENT_ALERT_QUEUE_SIZE")? {
            self.p2p.libtorrent.alert_queue_size = value;
        }
        if let Some(value) = parse_env_bool("PARADOWN_LIBTORRENT_DHT")? {
            self.p2p.libtorrent.enable_dht = value;
        }
        if let Some(value) = parse_env_bool("PARADOWN_LIBTORRENT_LSD")? {
            self.p2p.libtorrent.enable_lsd = value;
        }
        if let Some(value) = parse_env_bool("PARADOWN_LIBTORRENT_UPNP")? {
            self.p2p.libtorrent.enable_upnp = value;
        }
        if let Some(value) = parse_env_bool("PARADOWN_LIBTORRENT_NATPMP")? {
            self.p2p.libtorrent.enable_natpmp = value;
        }
        if let Some(value) = read_env("PARADOWN_LIBTORRENT_LISTEN_INTERFACES") {
            self.p2p.libtorrent.listen_interfaces = Some(value);
        }
        if let Some(value) = parse_env_bool("PARADOWN_SWARM_PROVIDERS_ENABLED")? {
            self.p2p.swarm.enabled = value;
        }
        if let Some(value) = read_env("PARADOWN_SWARM_PROVIDER_CACHE_DIR") {
            self.p2p.swarm.cache_dir = Some(value.into());
        }
        if let Some(value) = read_env("PARADOWN_TRACKER_LIST_URLS") {
            self.p2p.swarm.tracker_list_urls = parse_csv_env(&value);
        }
        if let Some(value) = parse_env_u64("PARADOWN_TRACKER_LIST_CACHE_TTL_SECS")? {
            self.p2p.swarm.tracker_list_cache_ttl_secs = value;
        }
        if let Some(value) = parse_env_u64("PARADOWN_TRACKER_LIST_TIMEOUT_SECS")? {
            self.p2p.swarm.tracker_list_timeout_secs = value;
        }
        if let Some(value) = read_env("PARADOWN_INDEX_URL_TEMPLATES") {
            self.p2p.swarm.index_providers = parse_csv_env(&value)
                .into_iter()
                .enumerate()
                .map(|(index, url_template)| SwarmIndexProviderConfig {
                    name: format!("env-index-{}", index + 1),
                    url_template,
                    input_kind: Default::default(),
                    cache_ttl_secs: self.p2p.swarm.tracker_list_cache_ttl_secs,
                    timeout_secs: self.p2p.swarm.tracker_list_timeout_secs,
                })
                .collect();
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

fn validate_p2p_config(p2p: &P2pConfig) -> Result<(), ConfigError> {
    if p2p.libtorrent.alert_queue_size == 0 {
        return Err(ConfigError::InvalidP2pConfig(
            "libtorrent alert_queue_size must be greater than 0".into(),
        ));
    }

    if p2p
        .libtorrent
        .listen_interfaces
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(ConfigError::InvalidP2pConfig(
            "libtorrent listen_interfaces cannot be blank".into(),
        ));
    }
    if p2p.swarm.tracker_list_cache_ttl_secs == 0 {
        return Err(ConfigError::InvalidP2pConfig(
            "swarm tracker_list_cache_ttl_secs must be greater than 0".into(),
        ));
    }
    if p2p.swarm.tracker_list_timeout_secs == 0 || p2p.swarm.discovery_timeout_secs == 0 {
        return Err(ConfigError::InvalidP2pConfig(
            "swarm provider timeouts must be greater than 0".into(),
        ));
    }
    for provider in &p2p.swarm.index_providers {
        if provider.name.trim().is_empty() {
            return Err(ConfigError::InvalidP2pConfig(
                "swarm index provider name cannot be blank".into(),
            ));
        }
        if provider.url_template.trim().is_empty() {
            return Err(ConfigError::InvalidP2pConfig(
                "swarm index provider url_template cannot be blank".into(),
            ));
        }
        if provider.cache_ttl_secs == 0 || provider.timeout_secs == 0 {
            return Err(ConfigError::InvalidP2pConfig(
                "swarm index provider cache_ttl_secs and timeout_secs must be greater than 0"
                    .into(),
            ));
        }
    }
    if p2p.swarm.limits.max_trackers == 0
        || p2p.swarm.limits.max_peers == 0
        || p2p.swarm.limits.max_web_seeds == 0
        || p2p.swarm.limits.max_locators == 0
        || p2p.swarm.limits.max_diagnostics == 0
    {
        return Err(ConfigError::InvalidP2pConfig(
            "swarm provider limits must be greater than 0".into(),
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

fn default_p2p_enabled() -> bool {
    true
}

fn default_storage_backend() -> Backend {
    Backend::Sqlite("./downloads.db".into())
}

fn default_file_conflict_strategy() -> FileConflictStrategy {
    FileConflictStrategy::Resume
}

/// 从环境中安全地读取一个变量，并进行清理。
///
/// 这个函数执行以下步骤：
/// 1. 尝试读取由 `key` 指定的环境变量。
/// 2. 如果变量不存在，直接返回 `None`。
/// 3. 如果变量存在，移除其值前后的所有空白字符。
/// 4. 如果清理后的值是空的，则返回 `None`。
/// 5. 否则，返回包含清理后值的 `Some(String)`。
///
/// # `String` vs `&str` (字符串 vs 字符串切片)
///
/// - **`String`**: 拥有所有权的数据类型。它在内存的堆上分配空间，是可变的、可增长的。
///   可以把它想象成一个你拥有的、可以随意修改的文本文件。
/// - **`&str`**: 一个“借用”的、不可变的视图（切片），指向存储在别处的字符串数据。
///   它不拥有数据，只是一个指针和长度。可以把它想象成一个指向书中某一页的只读便签。
///
/// # 参数
///
/// * `key`: 要读取的环境变量的名称。
///
/// # 返回值
///
/// - `Some(String)`: 如果环境变量存在且包含非空内容。
/// - `None`: 如果环境变量不存在，或者其值为空或只包含空白字符。
fn read_env(key: &str) -> Option<String> {
    // 1. `env::var(key)`: 尝试读取环境变量。返回 `Result<String, VarError>`。
    //    - `Ok(value)`: 如果成功，`value` 是一个拥有所有权的 `String` (在堆上分配)。
    //    - `Err(_)`: 如果失败（例如，变量未设置）。
    env::var(key)
        // 2. `.ok()`: 将 `Result` 转换为 `Option`。
        //    - `Ok(value)` 变为 `Some(value)`。
        //    - `Err(_)` 变为 `None`。
        //    这样，如果变量不存在，后续的链式调用就会短路并返回 `None`。
        .ok()
        // 3. `.map(|value| ...)`: 如果是 `Some(value)`，则对 `value` 执行闭包。
        //    - `value.trim()`: 移除字符串前后的空白字符。这步返回一个 `&str`（字符串切片），
        //      它只是一个指向原 `String` 内存的视图，并不创建新数据。
        //    - `.to_string()`: 在 `&str` 上调用。这一步是关键：它会分配一块全新的内存，
        //      然后将 `trim()` 返回的 `&str` 所指向的内容复制到新内存中，
        //      最终根据这块新内存创建一个全新的、拥有所有权的 `String`。
        .map(|value| value.trim().to_string())
        // 4. `.filter(|value| ...)`: 如果是 `Some(value)`，则应用一个条件。
        //    - `!value.is_empty()`: 检查清理后的字符串是否不为空。
        //    - 如果条件为 `true`，`Some(value)` 保持不变。
        //    - 如果条件为 `false`（字符串为空），则 `Some(value)` 变为 `None`。
        .filter(|value| !value.is_empty())
}

/// 解析一个环境变量为布尔值。
///
/// # `let-else` 语法
///
/// `let Some(value) = ... else { ... };` 是一种模式匹配的语法糖。
/// - 如果 `read_env(key)` 返回 `Some(value)`，则匹配成功，`value` 被绑定，程序继续。
/// - 如果返回 `None`，则匹配失败，`else` 块被执行。`else` 块必须发散（如 `return`），
///   这里它直接返回 `Ok(None)`，提前退出了函数。
///
/// # `as_str()` vs `to_string()`
///
/// - `value.to_ascii_lowercase()`: `value` 是 `String`，此方法返回一个新的、小写的 `String`。
/// - `.as_str()`: 在新的小写 `String` 上调用，返回一个 `&str` 切片。
///   这允许我们高效地与 `match` 分支中的 `&'static str` 进行比较，而无需再次分配内存。
fn parse_env_bool(key: &str) -> Result<Option<bool>, ConfigError> {
    // 尝试读取环境变量，如果不存在或为空，`let-else` 会让我们直接返回 `Ok(None)`。
    let Some(value) = read_env(key) else {
        return Ok(None);
    };

    // 对读取到的值进行不区分大小写的匹配。
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(Some(true)),
        "0" | "false" | "no" | "off" => Ok(Some(false)),
        // 如果值不是任何一个预期的布尔表示，则返回一个错误。
        _ => Err(ConfigError::InvalidEnvValue {
            key: key.to_string(),
            value, // `value` 的所有权在这里被转移到错误类型中。
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

fn parse_csv_env(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
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

            [p2p]
            enabled = true

            [p2p.libtorrent]
            alert_queue_size = 2048
            enable_dht = true
            enable_lsd = false
            enable_upnp = false
            enable_natpmp = false
            listen_interfaces = "0.0.0.0:6881"
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
        assert!(parsed.p2p.enabled);
        assert_eq!(parsed.p2p.libtorrent.alert_queue_size, 2048);
        assert!(!parsed.p2p.libtorrent.enable_lsd);
        assert_eq!(
            parsed.p2p.libtorrent.listen_interfaces.as_deref(),
            Some("0.0.0.0:6881")
        );
        assert!(matches!(
            parsed.file_conflict_strategy,
            FileConflictStrategy::Overwrite
        ));
    }

    #[test]
    fn parses_swarm_index_provider_config() {
        let config = r#"
            [p2p.swarm]

            [[p2p.swarm.index_providers]]
            name = "authorized-feed"
            url_template = "https://index.example/search?q={btih}"
            input_kind = "Feed"
            cache_ttl_secs = 300
            timeout_secs = 5
        "#;

        let parsed = config.parse::<Config>().unwrap();
        let provider = &parsed.p2p.swarm.index_providers[0];
        assert_eq!(provider.name, "authorized-feed");
        assert_eq!(
            provider.url_template,
            "https://index.example/search?q={btih}"
        );
        assert_eq!(provider.cache_ttl_secs, 300);
        assert_eq!(provider.timeout_secs, 5);
    }

    #[test]
    fn rejects_invalid_p2p_config() {
        let config = r#"
            [p2p.libtorrent]
            alert_queue_size = 0
        "#;

        let err = config.parse::<Config>().unwrap_err();
        assert!(matches!(
            err,
            ConfigLoadError::Config(super::ConfigError::InvalidP2pConfig(_))
        ));
    }

    #[test]
    fn rejects_invalid_swarm_index_provider_config() {
        let config = r#"
            [[p2p.swarm.index_providers]]
            name = " "
            url_template = "https://index.example/search?q={query}"
        "#;

        let err = config.parse::<Config>().unwrap_err();
        assert!(matches!(
            err,
            ConfigLoadError::Config(super::ConfigError::InvalidP2pConfig(_))
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
