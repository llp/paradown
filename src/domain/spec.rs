use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::fmt;

/// `DownloadSpec` (Download Specification) 代表一个下载任务的详细规格或定义。
///
/// 在编程中，`spec` 通常是 "specification"（规范、规格）的缩写。
/// `DownloadSpec` 类型封装了启动一个下载任务所需的所有必要信息，
/// 它可以是不同类型的资源（如 HTTP/HTTPS URL、FTP URL、Torrent 文件、Magnet 链接等）。
///
/// 这个枚举的每个变体都代表一种不同的下载源或下载方式，并包含该方式所需的具体数据。
///
/// 例如：
/// - `Http`, `Https`, `Ftp`: 包含资源的 URL。
/// - `TorrentFile`: 包含本地 Torrent 文件的路径。
/// - `Magnet`: 包含 Magnet 链接的 URI。
/// - `Metadata`: 包含一些元数据提示，用于发现资源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadSpec {
    /// HTTP 下载源。
    ///
    /// 这是结构体风格的枚举变体，`url` 字段保存完整的 HTTP URL。
    Http { url: String },
    /// HTTPS 下载源。
    ///
    /// 字段结构和 `Http` 一样，但协议不同，后续可以根据协议选择不同的传输逻辑。
    Https { url: String },
    /// FTP 下载源。
    ///
    /// `url` 保存完整 FTP 地址，例如 `ftp://example.com/file.bin`。
    Ftp { url: String },
    /// 本地 torrent 文件下载源。
    ///
    /// 这里保存的是本地 `.torrent` 文件路径，而不是网络 URL。
    TorrentFile { path: String },
    /// Magnet 链接下载源。
    ///
    /// Magnet URI 通常携带 info hash 等 P2P 元信息，可用于进入 swarm 发现流程。
    Magnet { uri: String },
    /// 只有元数据提示的下载源。
    ///
    /// 两个字段都是 `Option<String>`，表示它们都可能存在，也可能不存在：
    /// - `display_name`：用于展示或作为文件名提示的人类可读名称。
    /// - `info_hash`：P2P 场景中用于识别资源内容的 hash。
    Metadata {
        display_name: Option<String>,
        info_hash: Option<String>,
    },
}

impl DownloadSpec {
    /// 将用户输入的定位符解析成具体的下载规格。
    ///
    /// 参数类型是 `impl Into<String>`，表示调用者可以传入任何能转换成 `String` 的类型，
    /// 比如 `String` 或 `&str`。函数内部通过 `locator.into()` 统一转换成拥有所有权的 `String`。
    ///
    /// 返回类型是 `Result<Self, Error>`：
    /// - `Ok(Self::...)` 表示解析成功，并返回某个 `DownloadSpec` 枚举变体。
    /// - `Err(Error)` 表示解析失败，例如 URL 格式错误或协议不支持。
    ///
    /// `Self` 在 `impl DownloadSpec` 里等价于 `DownloadSpec`，
    /// 所以 `Ok(Self::Http { url: locator })` 就是返回 `Ok(DownloadSpec::Http { ... })`。
    ///
    /// 解析流程：
    /// - 先尝试把输入当作 URL 解析。
    /// - 如果 URL 解析成功，再根据 URL scheme 选择 `Http`、`Https`、`Ftp` 或 `Magnet`。
    /// - 如果 URL 解析失败的原因是“相对路径没有 base”，但字符串以 `.torrent` 结尾，
    ///   就把它当作本地 torrent 文件路径。
    /// - 其他错误继续转换为项目自己的 `Error` 类型返回。
    pub fn parse(locator: impl Into<String>) -> Result<Self, Error> {
        let locator = locator.into();
        match url::Url::parse(&locator) {
            Ok(parsed) => match parsed.scheme() {
                "http" => Ok(Self::Http { url: locator }),
                "https" => Ok(Self::Https { url: locator }),
                "ftp" => Ok(Self::Ftp { url: locator }),
                "magnet" => Ok(Self::Magnet { uri: locator }),
                other => Err(Error::UnsupportedProtocol(other.to_string())),
            },
            Err(url::ParseError::RelativeUrlWithoutBase) if locator.ends_with(".torrent") => {
                Ok(Self::TorrentFile { path: locator })
            }
            Err(err) => Err(err.into()),
        }
    }

    /// 返回当前下载规格的字符串定位符。
    ///
    /// 这个函数为 `DownloadSpec` 的不同变体提供一个统一的 `&str` 形式的定位符。
    /// 它只是借用内部字符串的数据，不创建新的 `String`，因此非常高效。
    ///
    /// # 逻辑分解
    ///
    /// - 对于 `Http`, `Https`, `Ftp`, `TorrentFile`, `Magnet` 变体，直接返回其内部的 `url`, `path` 或 `uri` 字段的 `&str` 引用。
    /// - 对于 `Metadata` 变体，逻辑稍微复杂，因为它包含两个 `Option<String>` 字段：`display_name` 和 `info_hash`。
    ///   其目的是尝试从 `info_hash` 获取定位符，如果 `info_hash` 不存在，则尝试从 `display_name` 获取，如果两者都不存在，则使用默认值 `"metadata"`。
    ///
    ///   具体分解 `info_hash.as_deref().or(display_name.as_deref()).unwrap_or("metadata")`：
    ///   1.  `info_hash.as_deref()`:
    ///       - `info_hash` 的类型是 `Option<String>`。
    ///       - `.as_deref()` 方法将 `Option<String>` 转换为 `Option<&str>`。
    ///       - 如果 `info_hash` 是 `Some(String)`，它会变成 `Some(&str)`（借用内部的字符串切片）。
    ///       - 如果 `info_hash` 是 `None`，它仍然是 `None`。
    ///       - 目的：获得一个 `Option<&str>`，这样我们就可以在不拥有 `String` 的情况下操作其内容。
    ///   2.  `.or(display_name.as_deref())`:
    ///       - 这是 `Option` 上的一个方法，用于提供一个备用 `Option`。
    ///       - 如果 `info_hash.as_deref()` 的结果是 `Some(...)`，那么 `or()` 会直接返回这个 `Some(...)`。
    ///       - 如果 `info_hash.as_deref()` 的结果是 `None`，那么 `or()` 会计算其参数 `display_name.as_deref()` 的值，并返回那个结果。
    ///       - 目的：实现“如果第一个选项有值就用第一个，否则用第二个”的逻辑。`display_name.as_deref()` 同样将 `Option<String>` 转换为 `Option<&str>`。
    ///   3.  `.unwrap_or("metadata")`:
    ///       - 这是 `Option` 上的一个方法，用于从 `Option` 中提取值，或者在 `Option` 为 `None` 时提供一个默认值。
    ///       - 如果前面的链式调用（`info_hash.as_deref().or(display_name.as_deref())`）的结果是 `Some(value)`，那么 `unwrap_or()` 会返回 `value`。
    ///       - 如果结果是 `None`（即 `info_hash` 和 `display_name` 都为 `None`），那么 `unwrap_or()` 会返回提供的默认值 `"metadata"`。
    ///       - 目的：确保总能返回一个 `&str`，即使所有可选字段都缺失。
    pub fn locator(&self) -> &str {
        match self {
            Self::Http { url } | Self::Https { url } | Self::Ftp { url } => url,
            Self::TorrentFile { path } => path,
            Self::Magnet { uri } => uri,
            Self::Metadata {
                display_name,
                info_hash,
            } => info_hash
                .as_deref()
                .or(display_name.as_deref())
                .unwrap_or("metadata"),
        }
    }

    /// 返回当前下载规格对应的协议名。
    ///
    /// 返回值类型是 `&'static str`，表示返回的是一个字符串切片引用，
    /// 并且这个引用指向程序整个运行期间都有效的字符串字面量，例如 `"http"`。
    /// 这里之所以可以写 `'static`，是因为本函数只返回固定的字符串字面量：
    /// `"http"`、`"https"`、`"ftp"` 等都被编译进程序本身，不依赖 `self` 里的字段。
    ///
    /// 如果把返回类型写成 `&str`，代码通常也能编译，因为 `&'static str`
    /// 可以自动缩短成普通的 `&str`。不过 `&str` 的含义更宽泛：
    /// 按 Rust 的生命周期省略规则，`pub fn scheme(&self) -> &str`
    /// 大致会被理解为“返回值的生命周期和 `self` 的借用生命周期有关”。
    /// 这在语义上像是在说返回值可能是从 `self` 内部借出来的。
    ///
    /// 与之对比，`locator(&self) -> &str` 就不能写成 `&'static str`，
    /// 因为 `locator` 可能返回 `url`、`path`、`uri` 等存放在 `self` 里的 `String` 字段。
    /// 一旦 `DownloadSpec` 被释放，这些字段也会被释放，所以它们的引用不能保证活到程序结束。
    ///
    /// 因此这里使用 `&'static str` 是一个更精确的 API 承诺：
    /// 调用者可以知道 `scheme()` 返回的是固定协议名常量，而不是借用了当前对象内部的数据。
    ///
    /// `Self::Http { .. }` 是枚举变体的模式匹配写法：
    /// - `Self` 表示当前枚举类型 `DownloadSpec`。
    /// - `Http { .. }` 表示匹配 `DownloadSpec::Http` 这个结构体风格的枚举变体。
    /// - `{ .. }` 表示忽略该变体里的所有字段；这里不需要读取 `url`，只需要知道它是哪种协议。
    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Http { .. } => "http",
            Self::Https { .. } => "https",
            Self::Ftp { .. } => "ftp",
            Self::TorrentFile { .. } => "torrent",
            Self::Magnet { .. } => "magnet",
            Self::Metadata { .. } => "metadata",
        }
    }

    /// 尝试从下载规格中推断一个适合作为文件名的提示。
    ///
    /// 返回类型是 `Option<String>`，表示“可能有文件名，也可能没有”：
    /// - `Some(name)`：成功推断出文件名提示。
    /// - `None`：没有足够信息推断文件名。
    ///
    /// HTTP、HTTPS、FTP 和 Magnet 都先通过 `self.locator()` 得到统一定位符，
    /// 再交给 `file_name_hint_from_locator` 从 URL path 的最后一段里提取文件名。
    ///
    /// `as_deref()` 常用于把 `Option<String>` 临时看作 `Option<&str>`。
    /// 例如测试里会用 `spec.file_name_hint().as_deref()`，
    /// 这样就能把 `Option<String>` 和 `Some("file.bin")` 这种 `Option<&str>` 直接比较。
    ///
    /// 对于 torrent 文件路径，这里使用 `std::path::Path` 处理路径，比手动按 `/` 切字符串更稳。
    /// `to_string_lossy()` 会把操作系统路径片段转换成可显示的字符串：
    /// 如果路径里有非 UTF-8 字节，它会用替代字符处理，而不是直接报错。
    pub fn file_name_hint(&self) -> Option<String> {
        match self {
            Self::Http { .. } | Self::Https { .. } | Self::Ftp { .. } | Self::Magnet { .. } => {
                file_name_hint_from_locator(self.locator())
            }
            Self::TorrentFile { path } => std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            Self::Metadata { display_name, .. } => display_name.clone(),
        }
    }

    /// 判断当前下载规格是否支持“源站发现”。
    ///
    /// `matches!` 是 Rust 标准库提供的宏，用来判断一个值是否匹配某个模式。
    /// 它的基本写法是 `matches!(表达式, 模式)`，返回值是 `bool`：
    /// - 如果表达式的值符合后面的模式，返回 `true`。
    /// - 如果不符合，返回 `false`。
    ///
    /// 这里的 `matches!(self, Self::Http { .. } | Self::Https { .. })`
    /// 等价于下面这种 `match` 写法：
    ///
    /// ```ignore
    /// match self {
    ///     Self::Http { .. } | Self::Https { .. } => true,
    ///     _ => false,
    /// }
    /// ```
    ///
    /// `|` 表示“或者”，所以这里的意思是：
    /// 只要当前值是 `Http` 或 `Https` 变体，就支持源站发现。
    /// `{ .. }` 表示忽略变体里的字段，因为这里只关心类型，不关心具体 URL。
    pub fn supports_origin_discovery(&self) -> bool {
        matches!(self, Self::Http { .. } | Self::Https { .. })
    }

    /// 判断当前下载规格是否支持“群组/节点发现”。
    ///
    /// 这里同样使用 `matches!` 宏把“模式匹配后返回布尔值”的逻辑写得更简洁。
    /// 它适合这种场景：只需要判断一个值是不是某几种 enum 变体之一，
    /// 不需要在每个分支里执行复杂逻辑。
    ///
    /// `matches!` 宏内部可以使用普通 `match` 支持的模式语法。
    /// 因此这里可以用 `|` 同时匹配多个变体：
    ///
    /// ```ignore
    /// Self::TorrentFile { .. } | Self::Magnet { .. } | Self::Metadata { .. }
    /// ```
    ///
    /// 这表示三种情况任意一种匹配都算成功：
    /// - `TorrentFile`：可以从 torrent 文件里获得 P2P 元信息。
    /// - `Magnet`：可以通过 magnet 链接发现 swarm。
    /// - `Metadata`：已有元数据提示，也可以进入 swarm 发现流程。
    pub fn supports_swarm_discovery(&self) -> bool {
        matches!(
            self,
            Self::TorrentFile { .. } | Self::Magnet { .. } | Self::Metadata { .. }
        )
    }

    /// 生成当前下载规格的身份标识 key。
    ///
    /// 这个 key 用来标识“这是不是同一个下载目标”，适合给任务去重、状态记录或缓存索引用。
    ///
    /// 返回类型是拥有所有权的 `String`，而不是 `&str`：
    /// - 对于 HTTP、HTTPS、FTP 和 Magnet，可以直接克隆内部字符串。
    /// - 对于 `TorrentFile` 和 `Metadata`，需要用 `format!` 组合出新的字符串。
    ///
    /// `clone()` 会复制一份 `String` 的内容，让返回值独立于 `self`。
    /// 这样调用者拿到 `identity_key()` 的结果后，即使原来的 `DownloadSpec` 被释放，
    /// 这个 key 仍然可以继续使用。
    ///
    /// `format!("torrent-file:{path}")` 是 Rust 的格式化字符串写法，
    /// 会把变量 `path` 插入到 `{path}` 位置，生成一个新的 `String`。
    ///
    /// `unwrap_or("")` 在这里表示：如果可选字段是 `None`，就用空字符串参与拼接。
    /// 这样即使 `Metadata` 缺少 `display_name` 或 `info_hash`，也总能生成稳定的 key。
    pub fn identity_key(&self) -> String {
        match self {
            Self::Http { url } | Self::Https { url } | Self::Ftp { url } => url.clone(),
            Self::TorrentFile { path } => format!("torrent-file:{path}"),
            Self::Magnet { uri } => uri.clone(),
            Self::Metadata {
                display_name,
                info_hash,
            } => format!(
                "metadata:{}:{}",
                info_hash.as_deref().unwrap_or(""),
                display_name.as_deref().unwrap_or("")
            ),
        }
    }
}

/// 从一个 URL 字符串里尝试提取文件名。
///
/// 例如 `https://example.com/download/file.bin` 的 path 最后一段是 `file.bin`，
/// 因此这个函数会返回 `Some("file.bin".to_string())`。
///
/// 函数返回 `Option<String>`：
/// - URL 解析失败时返回 `None`。
/// - URL path 没有可用的最后一段时返回 `None`。
/// - 成功提取文件名时返回 `Some(String)`。
///
/// 链式调用分解：
/// - `url::Url::parse(locator).ok()`：把 `Result<Url, _>` 转成 `Option<Url>`，
///   成功是 `Some(url)`，失败是 `None`。
/// - `.and_then(...)`：只有前一步是 `Some` 时才继续执行闭包；闭包也返回 `Option`。
/// - `url.path_segments()`：取得 URL path 的分段迭代器，例如 `/a/b.txt` 会得到 `a` 和 `b.txt`。
/// - `segments.next_back()`：从迭代器末尾取最后一段。
/// - `.filter(|segment| !segment.is_empty())`：过滤掉空字符串，避免 `/download/` 这种路径返回空文件名。
/// - `.map(|segment| segment.to_string())`：把借用的 `&str` 转成拥有所有权的 `String`。
///
/// 关于 `Result<Url, _>`、`Option<Url>` 以及 `.ok()` 的详细区别，
/// 见 `docs/rust/result-option.md`。
pub fn file_name_hint_from_locator(locator: &str) -> Option<String> {
    url::Url::parse(locator).ok().and_then(|url| {
        url.path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|segment| !segment.is_empty())
            .map(|segment| segment.to_string())
    })
}

/// 实现标准库的 `Display` trait，让 `DownloadSpec` 可以被格式化成字符串。
///
/// 有了这个实现，就可以使用：
///
/// ```ignore
/// println!("{spec}");
/// let text = spec.to_string();
/// ```
///
/// `fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result` 是 `Display` trait 要求的函数签名。
/// `fmt::Formatter<'_>` 里的 `'_` 是匿名生命周期，表示让编译器自动推断这个 formatter 引用的生命周期。
/// 最后通过 `write!` 宏把 `self.locator()` 写入 formatter。
impl fmt::Display for DownloadSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.locator())
    }
}

/// 支持把 `&str` 尝试转换成 `DownloadSpec`。
///
/// `TryFrom` 是标准库提供的转换 trait，适合“转换可能失败”的场景。
/// 与 `From` 不同，`TryFrom` 的返回值是 `Result`，可以携带错误。
///
/// 有了这个实现，就可以写：
///
/// ```ignore
/// let spec = DownloadSpec::try_from("https://example.com/file.bin")?;
/// ```
impl TryFrom<&str> for DownloadSpec {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// 支持把 `String` 尝试转换成 `DownloadSpec`。
///
/// 这个实现和 `TryFrom<&str>` 类似，只是输入已经是拥有所有权的 `String`。
/// 函数仍然复用 `Self::parse(value)`，让解析规则只维护一份。
impl TryFrom<String> for DownloadSpec {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::DownloadSpec;

    #[test]
    fn parses_http_locator() {
        let spec = DownloadSpec::parse("http://example.com/file.bin").unwrap();
        assert_eq!(spec.scheme(), "http");
        assert_eq!(spec.locator(), "http://example.com/file.bin");
    }

    #[test]
    fn parses_https_locator() {
        let spec = DownloadSpec::parse("https://example.com/file.bin").unwrap();
        assert_eq!(spec.scheme(), "https");
        assert_eq!(spec.file_name_hint().as_deref(), Some("file.bin"));
    }

    #[test]
    fn rejects_unsupported_protocols() {
        let spec = DownloadSpec::parse("magnet:?xt=urn:btih:deadbeef").unwrap();
        assert!(matches!(spec, DownloadSpec::Magnet { .. }));
    }

    #[test]
    fn keeps_http_torrent_links_as_http_specs() {
        let spec = DownloadSpec::parse("https://example.com/file.torrent").unwrap();
        assert!(matches!(spec, DownloadSpec::Https { .. }));
    }

    #[test]
    fn parses_local_torrent_files_as_torrent_specs() {
        let spec = DownloadSpec::parse("/tmp/archive.torrent").unwrap();
        assert!(matches!(spec, DownloadSpec::TorrentFile { .. }));
    }
}
