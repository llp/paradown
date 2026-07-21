use crate::domain::{DownloadSpec, HttpRequestOptions, HttpResourceIdentity};
use serde::{Deserialize, Serialize};

/// 下载源的类型。
///
/// `SourceKind` 比 `DownloadSpec` 更偏运行时视角：
/// - `DownloadSpec` 描述“用户最初给了什么下载规格”。
/// - `SourceKind` 描述“调度器/下载器当前可以使用哪一种来源”。
///
/// 例如一个 `Magnet` 规格本身不直接传输文件内容，但它可以发现出 `Peer`、`WebSeed`
/// 等新的下载源，所以这里会有比 `DownloadSpec` 更多的来源类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// HTTP 源站。
    Http,
    /// HTTPS 源站。
    Https,
    /// FTP 源站。
    Ftp,
    /// 本地 `.torrent` 文件，主要提供元数据。
    TorrentFile,
    /// Magnet 链接，主要用于发现 P2P 元数据和节点。
    Magnet,
    /// 已知的元数据提示，例如 info hash 或展示名。
    Metadata,
    /// BitTorrent tracker，负责帮助发现 peer。
    Tracker,
    /// DHT 网络来源，负责通过分布式哈希表发现 peer。
    Dht,
    /// P2P peer，可能直接提供文件分片。
    Peer,
    /// Web seed，通过 HTTP/HTTPS 提供 torrent 内容的下载源。
    WebSeed,
    /// 镜像源，通常表示同一资源的备用下载地址。
    Mirror,
    /// 离线缓存源，例如本地或内部缓存。
    OfflineCache,
}

/// 描述一个下载源具备哪些能力。
///
/// 这个结构体把“来源是什么”和“来源能做什么”拆开。
/// 例如 `SourceKind::Http` 表示来源类型是 HTTP，而 `SourceCapabilities`
/// 会进一步说明它是否支持范围请求、是否能直接传输文件内容、是否会动态变化等。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCapabilities {
    /// 是否可以通过这个源发现更多元数据。
    ///
    /// 例如 torrent 文件、magnet、tracker 这类来源更偏“发现信息”，而不是直接下载完整 payload。
    pub metadata_discovery: bool,
    /// 是否支持随机访问。
    ///
    /// 随机访问表示下载器可以按任意偏移读取内容，不必严格从头到尾顺序读取。
    pub random_access: bool,
    /// 是否支持 HTTP Range 之类的范围请求。
    ///
    /// 范围请求允许只下载文件的一段内容，是断点续传和多分片并发下载的重要能力。
    pub range_requests: bool,
    /// 是否支持上传。
    ///
    /// P2P peer 可能涉及上传，本项目的 HTTP/FTP 源站能力通常不支持上传。
    pub uploads: bool,
    /// 可用性是否会动态变化。
    ///
    /// P2P、tracker、DHT 等来源可能随时间发现新节点或失去节点，所以可用性更动态。
    pub dynamic_availability: bool,
}

impl SourceCapabilities {
    /// HTTP/HTTPS 源站的默认能力集合。
    ///
    /// HTTP 通常支持随机访问和 Range 请求，因此可以用于分片下载和断点续传。
    /// 这里不区分 HTTP 和 HTTPS，因为传输安全性不同，但下载能力基本一致。
    pub fn http_origin() -> Self {
        Self {
            metadata_discovery: false,
            random_access: true,
            range_requests: true,
            uploads: false,
            dynamic_availability: false,
        }
    }

    /// FTP 源站的默认能力集合。
    ///
    /// 这里把 FTP 视为可以随机访问，但不声明支持 Range 请求。
    /// `range_requests` 这个字段更贴近 HTTP Range 语义。
    pub fn ftp_origin() -> Self {
        Self {
            metadata_discovery: false,
            random_access: true,
            range_requests: false,
            uploads: false,
            dynamic_availability: false,
        }
    }

    /// 只用于元数据发现的来源能力集合。
    ///
    /// 这类来源本身不直接传输文件 payload，但可能帮助发现真正可下载的源。
    /// 例如 torrent 文件、magnet 链接、metadata hint 等。
    pub fn metadata_only() -> Self {
        Self {
            metadata_discovery: true,
            random_access: false,
            range_requests: false,
            uploads: false,
            dynamic_availability: true,
        }
    }
}

/// 一个具体下载源的完整描述。
///
/// `SourceDescriptor` 是调度和传输层使用的“来源记录”。
/// 它不仅说明来源类型，还包含唯一 id、定位符、请求参数、资源身份信息和能力集合。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDescriptor {
    /// 下载源的稳定 id。
    ///
    /// 通常由来源类型前缀加定位符组成，例如 `http::https://...`。
    /// 稳定 id 方便去重、查找和记录主源。
    pub id: String,
    /// 下载源类型。
    pub kind: SourceKind,
    /// 下载源定位符。
    ///
    /// 对 HTTP/HTTPS/FTP 来说通常是 URL；对 torrent 文件来说是路径；
    /// 对 peer 来说可能是 endpoint。
    pub locator: String,
    /// 是否只是元数据来源。
    ///
    /// `true` 表示这个来源主要用于发现，不直接传输最终文件内容。
    pub metadata_only: bool,
    /// HTTP 请求选项。
    ///
    /// 只有 HTTP/HTTPS 这类源站通常需要请求头、认证等 HTTP 选项，
    /// 所以这里用 `Option<HttpRequestOptions>` 表示它可能存在，也可能不存在。
    pub request: Option<HttpRequestOptions>,
    /// HTTP 资源身份信息。
    ///
    /// 例如 ETag、Last-Modified 或其他能确认远端资源身份的信息。
    /// 用 `Option` 是因为不是每个来源都能提供这类信息。
    pub resource_identity: Option<HttpResourceIdentity>,
    /// 当前来源具备的能力集合。
    pub capabilities: SourceCapabilities,
}

impl SourceDescriptor {
    /// 根据用户最初的 `DownloadSpec` 创建一个下载源描述。
    ///
    /// 这个函数是从“输入规格”进入“运行时下载源”的桥梁：
    /// - HTTP/HTTPS/FTP 会变成可以传输 payload 的源站。
    /// - TorrentFile/Magnet/Metadata 会变成 metadata-only 来源。
    ///
    /// 参数 `request: Option<HttpRequestOptions>` 只会保留给 HTTP/HTTPS/FTP 这类源站。
    /// 对 torrent、magnet、metadata 来说，HTTP 请求选项没有意义，所以会设为 `None`。
    ///
    /// `match spec` 这里匹配的是 `&DownloadSpec`，所以分支里的 `url`、`path`、`uri`
    /// 都是对 `spec` 内部字符串的借用。构造 `SourceDescriptor` 时需要拥有自己的 `String`，
    /// 因此这里会调用 `.clone()`。
    pub fn from_spec(spec: &DownloadSpec, request: Option<HttpRequestOptions>) -> Self {
        match spec {
            DownloadSpec::Http { url } => Self {
                id: format!("http::{url}"),
                kind: SourceKind::Http,
                locator: url.clone(),
                metadata_only: false,
                request,
                resource_identity: None,
                capabilities: SourceCapabilities::http_origin(),
            },
            DownloadSpec::Https { url } => Self {
                id: format!("https::{url}"),
                kind: SourceKind::Https,
                locator: url.clone(),
                metadata_only: false,
                request,
                resource_identity: None,
                capabilities: SourceCapabilities::http_origin(),
            },
            DownloadSpec::Ftp { url } => Self {
                id: format!("ftp::{url}"),
                kind: SourceKind::Ftp,
                locator: url.clone(),
                metadata_only: false,
                request,
                resource_identity: None,
                capabilities: SourceCapabilities::ftp_origin(),
            },
            DownloadSpec::TorrentFile { path } => Self {
                id: format!("torrent::{path}"),
                kind: SourceKind::TorrentFile,
                locator: path.clone(),
                metadata_only: true,
                request: None,
                resource_identity: None,
                capabilities: SourceCapabilities::metadata_only(),
            },
            DownloadSpec::Magnet { uri } => Self {
                id: format!("magnet::{uri}"),
                kind: SourceKind::Magnet,
                locator: uri.clone(),
                metadata_only: true,
                request: None,
                resource_identity: None,
                capabilities: SourceCapabilities::metadata_only(),
            },
            DownloadSpec::Metadata {
                display_name,
                info_hash,
            } => {
                // Metadata 可能缺少 info_hash 或 display_name，因此按优先级生成稳定 key：
                // 先用 info_hash，其次用 display_name，最后退回到固定字符串 "metadata"。
                let stable_key = info_hash
                    .clone()
                    .or_else(|| display_name.clone())
                    .unwrap_or_else(|| "metadata".into());
                Self {
                    id: format!("metadata::{stable_key}"),
                    kind: SourceKind::Metadata,
                    locator: stable_key,
                    metadata_only: true,
                    request: None,
                    resource_identity: None,
                    capabilities: SourceCapabilities::metadata_only(),
                }
            }
        }
    }

    /// 创建一个 tracker 来源。
    ///
    /// 参数使用 `impl Into<String>`，表示既可以传入 `String`，也可以传入 `&str`。
    /// 函数内部用 `url.into()` 取得拥有所有权的 `String`，方便存入结构体。
    ///
    /// Tracker 主要用于发现 peer，本身不直接传输最终文件内容，所以是 metadata-only。
    pub fn tracker(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            id: format!("tracker::{url}"),
            kind: SourceKind::Tracker,
            locator: url,
            metadata_only: true,
            request: None,
            resource_identity: None,
            capabilities: SourceCapabilities::metadata_only(),
        }
    }

    /// 创建一个 peer 来源。
    ///
    /// `endpoint` 通常表示 peer 的连接地址。
    /// 当前这里仍然把 peer 标成 metadata-only，说明它在此阶段更多参与发现/登记流程；
    /// 后续如果 peer 直接参与分片传输，可以再调整它的能力描述。
    pub fn peer(endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();
        Self {
            id: format!("peer::{endpoint}"),
            kind: SourceKind::Peer,
            locator: endpoint,
            metadata_only: true,
            request: None,
            resource_identity: None,
            capabilities: SourceCapabilities::metadata_only(),
        }
    }

    /// 创建一个 web seed 来源。
    ///
    /// Web seed 是 BitTorrent 生态里的一种 HTTP/HTTPS 下载源：
    /// 它不是普通 peer，但可以通过 Web 服务器提供 torrent 内容。
    ///
    /// 因为 web seed 可以直接传输 payload，所以 `metadata_only` 是 `false`。
    /// 它也通常支持随机访问和范围请求，适合与 P2P 下载互补。
    pub fn web_seed(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            id: format!("web-seed::{url}"),
            kind: SourceKind::WebSeed,
            locator: url,
            metadata_only: false,
            request: None,
            resource_identity: None,
            capabilities: SourceCapabilities {
                metadata_discovery: false,
                random_access: true,
                range_requests: true,
                uploads: false,
                dynamic_availability: true,
            },
        }
    }

    /// 给当前来源附加 HTTP 资源身份信息，并返回更新后的 `Self`。
    ///
    /// 这里的参数是 `mut self`，表示函数拿走当前对象的所有权，并允许在函数体内修改它。
    /// 修改完成后再返回 `self`，因此可以链式调用：
    ///
    /// ```ignore
    /// let source = SourceDescriptor::from_spec(&spec, request).with_identity(identity);
    /// ```
    ///
    /// 这种写法常见于 builder 风格 API。
    pub fn with_identity(mut self, resource_identity: HttpResourceIdentity) -> Self {
        self.resource_identity = Some(resource_identity);
        self
    }

    /// 判断这个来源是否支持范围请求。
    ///
    /// 这是一个小的便捷方法，调用者不需要直接访问 `capabilities.range_requests` 字段。
    pub fn supports_range_requests(&self) -> bool {
        self.capabilities.range_requests
    }

    /// 判断这个来源是否可以直接传输最终文件内容。
    ///
    /// 当前规则很直接：只要不是 metadata-only 来源，就认为可以传输 payload。
    pub fn can_transfer_payload(&self) -> bool {
        !self.metadata_only
    }
}

/// 一组下载源。
///
/// 一个下载任务可能有多个来源：
/// - 用户最初提供的主 URL。
/// - 后续发现的 mirror。
/// - P2P 场景里发现的 peer、web seed 等。
///
/// `SourceSet` 负责保存这些来源，并记录哪个来源是 primary。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSet {
    /// 主下载源的 id。
    ///
    /// 使用 `Option<String>` 是因为一个集合可能暂时没有主源。
    pub primary_id: Option<String>,
    /// 当前已知的所有下载源。
    pub sources: Vec<SourceDescriptor>,
}

impl SourceSet {
    /// 创建一个只包含单个主源的来源集合。
    ///
    /// `source.id.clone()` 是因为：
    /// - `primary_id` 需要保存一份 id。
    /// - `sources` 也要继续拥有完整的 `source`。
    ///
    /// 如果不克隆 id，`source` 被移动进 `vec![source]` 后，就不能再读取它的字段了。
    pub fn single_primary(source: SourceDescriptor) -> Self {
        Self {
            primary_id: Some(source.id.clone()),
            sources: vec![source],
        }
    }

    /// 根据 `DownloadSpec` 创建一个只有主源的 `SourceSet`。
    ///
    /// 这是 `SourceDescriptor::from_spec` 和 `SourceSet::single_primary` 的组合快捷方法。
    pub fn for_spec(spec: &DownloadSpec, request: Option<HttpRequestOptions>) -> Self {
        Self::single_primary(SourceDescriptor::from_spec(spec, request))
    }

    /// 添加一个新来源，但避免重复 id。
    ///
    /// `iter().any(...)` 会遍历已有来源，只要有一个来源 id 和新来源相同，就返回 `true`。
    /// 前面的 `!` 表示“如果不存在相同 id”，才把新来源 push 进去。
    ///
    /// 这里的去重依据是 `SourceDescriptor.id`，不是整个结构体是否完全相等。
    pub fn push_unique(&mut self, source: SourceDescriptor) {
        if !self.sources.iter().any(|existing| existing.id == source.id) {
            self.sources.push(source);
        }
    }

    /// 判断来源集合是否为空。
    ///
    /// 这是对 `self.sources.is_empty()` 的便捷封装。
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// 返回当前来源数量。
    ///
    /// 返回类型 `usize` 是 Rust 中常用于集合长度和索引的无符号整数类型。
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// 获取主来源的不可变引用。
    ///
    /// 返回 `Option<&SourceDescriptor>`，因为主来源可能不存在：
    /// - `Some(&source)`：找到了主来源。
    /// - `None`：集合里没有任何来源。
    ///
    /// 查找逻辑：
    /// - 如果 `primary_id` 存在，就按 id 在 `sources` 中查找对应来源。
    /// - 如果找不到或没有 `primary_id`，就退回到第一个来源。
    ///
    /// `as_ref()` 会把 `Option<String>` 转成 `Option<&String>`，
    /// 这样可以借用 id，而不是把 `String` 从 `self` 里移动出来。
    ///
    /// `and_then(...)` 表示当前一步是 `Some` 时继续查找，查找结果本身也是 `Option`。
    /// `or_else(...)` 表示前面没找到时，再尝试使用第一个来源作为 fallback。
    pub fn primary(&self) -> Option<&SourceDescriptor> {
        self.primary_id
            .as_ref()
            .and_then(|id| self.sources.iter().find(|source| &source.id == id))
            .or_else(|| self.sources.first())
    }

    /// 获取主来源的可变引用。
    ///
    /// 返回 `Option<&mut SourceDescriptor>`，因为集合可能为空，或者指定的 `primary_id` 找不到。
    ///
    /// 可变引用 `&mut` 表示调用者可以修改返回的 `SourceDescriptor`。
    /// Rust 同一时间只允许一个可变引用存在，这能避免数据竞争和别名修改问题。
    ///
    /// 这里先尝试按 `primary_id` 找；如果没有设置 `primary_id`，才返回第一个来源的可变引用。
    pub fn primary_mut(&mut self) -> Option<&mut SourceDescriptor> {
        if let Some(primary_id) = self.primary_id.as_ref() {
            return self
                .sources
                .iter_mut()
                .find(|source| &source.id == primary_id);
        }

        self.sources.first_mut()
    }

    /// 根据来源 id 查找一个下载源。
    ///
    /// `find` 返回第一个满足条件的元素引用；如果没有匹配项，返回 `None`。
    pub fn get(&self, source_id: &str) -> Option<&SourceDescriptor> {
        self.sources.iter().find(|source| source.id == source_id)
    }

    /// 返回所有可以直接传输 payload 的来源。
    ///
    /// 返回类型是 `Vec<&SourceDescriptor>`：
    /// - `Vec` 表示收集出一个列表。
    /// - `&SourceDescriptor` 表示列表里保存的是对原始来源的引用，不复制整个来源对象。
    ///
    /// `filter(|source| source.can_transfer_payload())` 会保留所有非 metadata-only 的来源。
    pub fn active_transfer_sources(&self) -> Vec<&SourceDescriptor> {
        self.sources
            .iter()
            .filter(|source| source.can_transfer_payload())
            .collect()
    }

    /// 替换当前主来源。
    ///
    /// 这个方法会先把 `primary_id` 更新为新来源的 id。
    /// 然后检查 `sources` 里是否已经存在同 id 的来源：
    /// - 如果存在，就用新的 `source` 覆盖旧来源。
    /// - 如果不存在，就把新的 `source` 插入到列表开头。
    ///
    /// `if let Some(existing) = ...` 是 Rust 中常见的“只处理某一种匹配结果”的写法。
    /// 这里表示：如果找到了已有来源，就进入代码块并得到它的可变引用。
    ///
    /// `*existing = source;` 里的 `*existing` 是解引用。
    /// 因为 `existing` 的类型是 `&mut SourceDescriptor`，要替换它指向的实际值，
    /// 需要先用 `*` 访问引用背后的对象。
    pub fn replace_primary(&mut self, source: SourceDescriptor) {
        self.primary_id = Some(source.id.clone());
        if let Some(existing) = self
            .sources
            .iter_mut()
            .find(|existing| existing.id == source.id)
        {
            *existing = source;
            return;
        }

        self.sources.insert(0, source);
    }
}

/// `SourceSet` 相关单元测试。
///
/// 这些测试重点验证：
/// - 能否从 `DownloadSpec` 构建主来源集合。
/// - 替换主来源时是否会按 id 覆盖已有来源。
#[cfg(test)]
mod tests {
    use super::{SourceDescriptor, SourceKind, SourceSet};
    use crate::domain::DownloadSpec;

    /// 验证 HTTPS 下载规格可以构建成只包含一个主来源的 `SourceSet`。
    ///
    /// `map(|source| source.kind.clone())` 表示：
    /// 如果 `primary()` 返回 `Some(source)`，就把里面的 `SourceKind` 克隆出来用于断言。
    /// 如果 `primary()` 返回 `None`，整个表达式仍然是 `None`。
    #[test]
    fn builds_single_http_source_set_from_spec() {
        let spec = DownloadSpec::parse("https://example.com/file.bin").unwrap();
        let sources = SourceSet::for_spec(&spec, None);

        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources.primary().map(|source| source.kind.clone()),
            Some(SourceKind::Https)
        );
        assert_eq!(
            sources
                .primary()
                .map(|source| source.locator.clone())
                .as_deref(),
            Some("https://example.com/file.bin")
        );
    }

    /// 验证 `replace_primary` 会按相同 id 替换已有主来源。
    ///
    /// 测试里构造的 `replacement` 和原始来源使用同一个 id，
    /// 但 locator 换成了 CDN 地址。替换后如果 `primary().locator` 是 CDN 地址，
    /// 就说明原来的来源被成功覆盖，而不是新增了一个重复来源。
    #[test]
    fn replaces_existing_primary_source_by_id() {
        let mut sources = SourceSet::single_primary(SourceDescriptor::from_spec(
            &DownloadSpec::parse("https://example.com/a.bin").unwrap(),
            None,
        ));
        let replacement = SourceDescriptor {
            id: "https::https://example.com/a.bin".into(),
            kind: SourceKind::Https,
            locator: "https://cdn.example.com/a.bin".into(),
            metadata_only: false,
            request: None,
            resource_identity: None,
            capabilities: super::SourceCapabilities::http_origin(),
        };

        sources.replace_primary(replacement);

        assert_eq!(
            sources
                .primary()
                .map(|source| source.locator.clone())
                .as_deref(),
            Some("https://cdn.example.com/a.bin")
        );
    }
}
