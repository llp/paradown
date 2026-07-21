use crate::config::{ConfigError, ConfigLoadError};
use reqwest::header::ToStrError;
use serde::{Deserialize, Serialize};
use std::error::Error as StdError;
use std::io;
use std::num::ParseIntError;
use thiserror::Error;
use tokio::sync::AcquireError;
use tokio::task::JoinError;

/// 项目统一错误类型。
///
/// 这里用 `enum` 而不是 `struct`，是因为错误天然是“多种情况中的一种”：
/// IO 错误、HTTP 错误、配置错误、任务取消等，每一种都可以作为一个 enum 变体。
///
/// `#[derive(...)]` 会为 `Error` 自动生成一批 trait 实现：
/// - `Debug`：支持 `{:?}` 调试输出。
/// - `Clone`：支持显式复制一份错误值。
/// - `Serialize` / `Deserialize`：支持用 serde 序列化和反序列化。
/// - `thiserror::Error`：来自 `thiserror` 的 derive 宏，会生成标准库错误 trait 和 `Display` 实现。
///
/// 注意 `Error` 这个名字同时出现在两处：
/// - `use thiserror::Error;` 引入的是 derive 宏名。
/// - `pub enum Error` 定义的是本项目自己的错误类型。
/// Rust 的宏命名空间和类型命名空间是分开的，所以这里不会冲突。
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum Error {
    /// 元组风格 enum 变体：`Io(String)`。
    ///
    /// 它没有字段名，只有字段位置。`#[error("IO error: {0}")]` 里的 `{0}`
    /// 表示格式化时使用第 0 个字段，也就是内部的 `String`。
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

/// 手动实现从 `url::ParseError` 到项目 `Error` 的转换。
///
/// `impl From<A> for B` 的含义是：告诉 Rust 如何把 `A` 转成 `B`。
/// 一旦有了这个实现，很多地方就可以使用：
///
/// ```ignore
/// let value = may_return_url_parse_error()?;
/// ```
///
/// 如果当前函数返回 `Result<_, Error>`，而被 `?` 的表达式返回 `Result<_, url::ParseError>`，
/// 编译器会尝试调用 `Error::from(err)` 把错误转换成项目统一错误类型。
///
/// 这里返回 `Self`，在 `impl From<url::ParseError> for Error` 中，
/// `Self` 就等价于 `Error`。
impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Self {
        Error::UrlParseError(err.to_string())
    }
}

/// 这些 `From` 实现共同组成项目的错误转换层。
///
/// 每个实现都遵循同一个模式：
///
/// ```ignore
/// impl From<外部错误类型> for Error {
///     fn from(err: 外部错误类型) -> Self {
///         Error::某个变体(err.to_string())
///     }
/// }
/// ```
///
/// 这样做的语法价值是让 `?` 更好用：底层库错误可以自动汇总到项目自己的 `Error`。
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

/// `Box<dyn StdError>` 是一个 trait object。
///
/// - `dyn StdError` 表示“某个实现了 `std::error::Error` trait 的具体类型”，
///   但这个具体类型在编译期不写死，而是在运行时通过动态分发调用。
/// - `Box<...>` 把这个动态大小的 trait object 放到堆上，用一个固定大小的指针持有它。
///
/// 这里的 `StdError` 来自 `use std::error::Error as StdError;`，
/// `as StdError` 是重命名导入，避免和本文件里的项目错误类型 `Error` 撞名。
impl From<Box<dyn StdError>> for Error {
    fn from(err: Box<dyn StdError>) -> Self {
        Error::Other(err.to_string())
    }
}

/// 从拥有所有权的 `String` 转成项目错误。
///
/// 参数名 `s: String` 表示函数会取得这个字符串的所有权，
/// 所以可以直接移动进 `Error::Other(s)`，不需要 clone。
impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Other(s)
    }
}

/// 从借用的字符串切片 `&str` 转成项目错误。
///
/// 因为 `&str` 不拥有字符串内容，不能直接存进需要拥有数据的 `Error::Other(String)`。
/// 所以这里调用 `.to_string()` 创建一份拥有所有权的 `String`。
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
