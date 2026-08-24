//! HTTP 领域模型与配置类型。
//!
//! 本模块不发送网络请求，而是描述 HTTP 子系统需要的数据：
//! - `HttpResourceIdentity` 保存探测阶段得到的最终 URL、ETag 和 Last-Modified，
//!   用于判断远端资源是否仍适合断点续传；
//! - `HttpRequestOptions` 描述每个请求可携带的 Header、Cookie、认证和 User-Agent；
//! - `HttpClientOptions` 描述创建共享 `reqwest::Client` 时使用的代理、Cookie Store 和 TLS；
//! - `HttpConfig` 把 Client 级配置与 Request 级配置组合成完整 HTTP 配置。
//!
//! 这些类型实现了 Serde 序列化/反序列化，既可以出现在配置文件中，也可以作为
//! Paradown 公共 Rust API 的参数。实际应用配置的代码位于 `runtime` 模块。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 能够标识一次 HTTP 下载目标的远端资源信息。
///
/// 协议探针会从响应中填充这些字段，任务持久化后可在恢复下载时与新的探测结果比较。
/// `Option` 表示服务器可能没有提供对应响应头；缺少验证器时，调用方不能仅凭本结构
/// 证明本地已有字节仍属于当前远端资源。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResourceIdentity {
    /// 跟随 HTTP 重定向后的最终 URL。
    ///
    /// 它可用于诊断和文件名推断，但 `validator_changed` 不把 URL 本身当作内容验证器。
    pub resolved_url: Option<String>,
    /// 原样保存的 `ETag` 响应头值，优先用于续传校验。
    pub entity_tag: Option<String>,
    /// 原样保存的 `Last-Modified` 响应头值；没有 ETag 时作为后备验证器。
    pub last_modified: Option<String>,
}

impl HttpResourceIdentity {
    /// 返回可用于 `If-Range` 的首选验证器。
    ///
    /// ETag 通常比修改时间更精确，因此优先级为 `entity_tag`，其次才是
    /// `last_modified`。`as_deref()` 把 `Option<String>` 借用成 `Option<&str>`，
    /// 不复制字符串。
    pub fn resume_validator(&self) -> Option<&str> {
        self.entity_tag.as_deref().or(self.last_modified.as_deref())
    }

    /// 是否至少保存了一个可以尝试验证续传的 ETag 或 Last-Modified。
    pub fn has_resume_validator(&self) -> bool {
        self.resume_validator().is_some()
    }

    /// 比较已保存身份 (`self`) 与最新探测身份 (`fresh`) 的首选验证器是否发生变化。
    ///
    /// 比较规则与 `resume_validator` 的优先级一致：
    /// - 旧、新两边都有 ETag 时，只比较 ETag；
    /// - 旧数据有 ETag 而新响应不再提供 ETag 时，视为变化；
    /// - 旧数据没有 ETag 时，再按同样规则比较 Last-Modified；
    /// - 旧数据原本没有任何对应验证器时，不会仅因为新响应新增验证器而判定变化。
    ///
    /// 返回 `true` 表示已有字节不能再被安全地视为同一远端实体，调用方应放弃直接续传。
    pub fn validator_changed(&self, fresh: &Self) -> bool {
        // ETag 是首选验证器；一旦旧、新两边都有值，就不再比较 Last-Modified。
        match (self.entity_tag.as_deref(), fresh.entity_tag.as_deref()) {
            (Some(current), Some(next)) => current != next,
            (Some(_), None) => true,
            // 旧身份没有 ETag 时，退回到精度较低的 Last-Modified。
            _ => match (
                self.last_modified.as_deref(),
                fresh.last_modified.as_deref(),
            ) {
                (Some(current), Some(next)) => current != next,
                (Some(_), None) => true,
                _ => false,
            },
        }
    }
}

/// 一条用户自定义的 HTTP 请求头。
///
/// 字段保存为字符串以方便配置文件和公共 API 使用；真正构造请求时，`runtime` 模块
/// 会把它们解析为 reqwest/http 的 HeaderName 与 HeaderValue，非法名称或值会返回错误。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHeader {
    /// Header 名称，例如 `Accept` 或 `X-Api-Key`。
    pub name: String,
    /// Header 值。此处不提前解析或规范化。
    pub value: String,
}

/// 单个 HTTP 请求使用的认证方式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpAuth {
    /// HTTP Basic Authentication。
    Basic {
        /// Basic Auth 用户名。
        username: String,
        /// 可选密码；`None` 会原样传给 reqwest 的 Basic Auth 构造逻辑。
        password: Option<String>,
    },
    /// HTTP Bearer Token Authentication。
    Bearer {
        /// 不包含 `Bearer ` 前缀的 token；reqwest 在构造 Authorization 头时添加前缀。
        token: String,
    },
}

/// 应用于一次 HTTP 请求的可选参数。
///
/// 这组配置既可作为任务级默认值，也可挂在某个具体下载源上。传输驱动构造 GET 请求、
/// 协议探针构造 HEAD/Range 请求时都会通过同一个辅助函数应用这些参数。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequestOptions {
    /// 自定义请求头列表。缺少配置字段时由 Serde 反序列化为空列表。
    #[serde(default)]
    pub headers: Vec<HttpHeader>,
    /// `Cookie` 请求头的完整值；不同于下面 Client 级的自动 Cookie Store。
    pub cookie: Option<String>,
    /// 可选的 Basic 或 Bearer 认证配置。
    pub auth: Option<HttpAuth>,
    /// 可选的 `User-Agent` 请求头值。
    pub user_agent: Option<String>,
}

impl HttpRequestOptions {
    /// 将基础请求配置与一组更具体的覆盖配置合并，返回新的配置值。
    ///
    /// 合并规则：
    /// - Header 列表按“基础在前、覆盖在后”的顺序直接拼接，不按名称去重；
    /// - Cookie、认证和 User-Agent 在 `overrides` 为 `Some` 时替换基础值；
    /// - `overrides` 对应字段为 `None` 时保留基础值；
    /// - 两个输入都只被借用，方法通过克隆构造独立结果，不修改任何一方。
    pub fn merged(&self, overrides: &Self) -> Self {
        // Header 允许同名多值，所以这里保留双方全部条目，而不是覆盖或去重。
        let mut headers = self.headers.clone();
        headers.extend(overrides.headers.iter().cloned());

        Self {
            headers,
            // `Option::or_else` 只在覆盖值为空时才克隆基础值。
            cookie: overrides.cookie.clone().or_else(|| self.cookie.clone()),
            auth: overrides.auth.clone().or_else(|| self.auth.clone()),
            user_agent: overrides
                .user_agent
                .clone()
                .or_else(|| self.user_agent.clone()),
        }
    }

    /// 判断是否完全没有需要额外应用到请求上的参数。
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
            && self.cookie.is_none()
            && self.auth.is_none()
            && self.user_agent.is_none()
    }
}

/// 创建 HTTP Client 时使用的代理配置。
///
/// 环境代理由 reqwest 的默认行为处理；显式 HTTP/HTTPS 代理和绕过规则由
/// `runtime::apply_proxy_options` 添加到 ClientBuilder。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyOptions {
    /// 是否允许 reqwest 读取系统代理环境变量；默认开启。
    ///
    /// 设为 `false` 时会调用 `ClientBuilder::no_proxy()` 禁用环境代理自动发现。
    #[serde(default = "default_true")]
    pub use_env_proxy: bool,
    /// HTTP 请求使用的显式代理 URL。
    pub http_proxy: Option<String>,
    /// HTTPS 请求使用的显式代理 URL。
    pub https_proxy: Option<String>,
    /// 应绕过上述显式代理的主机匹配串，格式由 reqwest 的 `NoProxy` 解析。
    pub no_proxy: Option<String>,
}

impl Default for ProxyOptions {
    /// 默认保留 reqwest 的环境代理发现，不设置任何显式代理或额外绕过规则。
    fn default() -> Self {
        Self {
            use_env_proxy: true,
            http_proxy: None,
            https_proxy: None,
            no_proxy: None,
        }
    }
}

/// 创建 HTTP Client 时使用的 TLS 配置。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsOptions {
    /// 是否接受证书验证失败的服务端证书。
    ///
    /// 默认是 `false`。开启会削弱服务端身份校验，存在中间人攻击风险，只应在明确
    /// 理解风险的受控环境中使用。
    #[serde(default)]
    pub insecure_skip_verify: bool,
    /// 要额外加入信任根集合的 PEM CA 证书文件路径，不会替换内置系统信任根。
    pub ca_certificate_pem: Option<PathBuf>,
    /// 客户端身份 PEM 文件路径，用于双向 TLS；文件需包含 reqwest/rustls 可解析的
    /// 客户端证书与私钥。
    pub client_identity_pem: Option<PathBuf>,
}

/// 构造共享 `reqwest::Client` 时使用的长期配置。
///
/// 与 `HttpRequestOptions` 不同，这些选项在 Manager 创建 HTTP Client 时应用一次，
/// 之后由协议探针和所有 HTTP Worker 共享。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpClientOptions {
    /// 环境代理、显式代理和代理绕过规则。
    #[serde(default)]
    pub proxy: ProxyOptions,
    /// 是否启用自动 Cookie Store，使响应中的 Cookie 可用于后续请求。
    #[serde(default)]
    pub cookie_store: bool,
    /// 可选 Cookie Jar 文件路径。
    ///
    /// 只有 `cookie_store` 为 `true` 时才会读取该文件；配置路径后，运行时也会保存
    /// 会话 Cookie。启用 Cookie Store 但不提供路径时，仅使用进程内存中的临时存储。
    pub cookie_jar_path: Option<PathBuf>,
    /// TLS 证书验证、自定义 CA 和客户端身份配置。
    #[serde(default)]
    pub tls: TlsOptions,
}

/// Paradown 的完整 HTTP 配置入口。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpConfig {
    /// Client 生命周期级配置：代理、Cookie Store 和 TLS。
    #[serde(default)]
    pub client: HttpClientOptions,
    /// 默认请求级配置：Header、Cookie、认证和 User-Agent。
    ///
    /// 创建具体任务时还可以提供更具体的 `HttpRequestOptions`，再通过 `merged()`
    /// 覆盖这里的标量设置并追加 Header。
    #[serde(default)]
    pub request: HttpRequestOptions,
}

/// Serde 字段默认值辅助函数。
///
/// `bool::default()` 是 `false`，但环境代理的产品默认值需要为 `true`，因此
/// `ProxyOptions::use_env_proxy` 使用 `#[serde(default = "default_true")]` 指向本函数。
fn default_true() -> bool {
    true
}
