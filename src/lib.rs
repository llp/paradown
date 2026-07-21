//! `src/lib.rs` 是这个包的 library crate 根文件。
//!
//! 一个 Cargo package 可以同时有 library crate 和 binary crate：
//! - `src/lib.rs` 编译成库，供项目内部的二进制或外部使用者调用。
//! - `src/main.rs` 编译成可执行程序。
//!
//! Rust 的模块系统从 crate 根开始组织代码。这里的 `mod xxx;` 不是“导入包”，
//! 而是声明“这个 crate 里有一个名为 xxx 的模块”，编译器会去找 `src/xxx.rs`
//! 或 `src/xxx/mod.rs`。
//!
//! 本文件下面分成两层：
//! - `mod` / `pub mod`：把源码文件纳入当前 crate 的模块树。
//! - `pub use`：把内部模块里的类型重新导出，形成更方便使用的公共 API。

// 没有 `pub` 的 `mod` 是私有模块：它属于当前 crate 的内部实现细节。
// 其他模块仍可通过 `crate::checksum` 等路径在 crate 内访问它，但外部使用者不能直接访问。
mod checksum;
mod config;
mod coordinator;
mod diagnostics;
// `pub mod` 会把整个模块作为公共 API 暴露出去。
// 使用者可以通过 `paradown::discovery::...` 访问其中的公开项。
pub mod discovery;
mod domain;
pub mod download;
mod error;
mod events;
mod job;
pub mod p2p;
mod payload;
mod protocol_probe;
mod rate_limiter;
mod recovery;
pub mod repository;
mod request;
mod runtime;
mod scheduler;
mod stats;
mod status;
pub mod storage;
mod transfer;
mod worker;

// `pub use` 是 re-export（重新导出）。
// `checksum` 模块本身是私有的，但这里把其中的公开类型重新导出到 crate 根。
// 这样外部使用者可以写 `paradown::Checksum`，不用知道它实际定义在 `checksum.rs`。
pub use checksum::{Checksum, ChecksumAlgorithm};
// 花括号里的列表是 use path 的分组写法，避免重复写 `config::`。
// 这组 re-export 把配置相关类型集中放到 `paradown::...` 这一层 API。
pub use config::{
    Config, ConfigBuilder, ConfigError, ConfigLoadError, FileConflictStrategy, LogLevel, P2pConfig,
    ProgressThrottleConfig, RetryConfig,
};
// 当 `pub use` 指向一个 `pub mod` 时，它仍然有价值：
// 使用者既可以走 `paradown::discovery::TorrentDiscoveryOptions`，
// 也可以走更短的 `paradown::TorrentDiscoveryOptions`。
pub use discovery::{
    TorrentDiscoveryCandidate, TorrentDiscoveryInputKind, TorrentDiscoveryKind,
    TorrentDiscoveryOptions, discover_torrent_candidates,
};
// `domain` 模块是私有模块，但它里面定义了大量领域类型。
// 通过 `pub use domain::{...}`，库作者可以隐藏内部文件结构，只暴露稳定类型名。
pub use domain::{
    BlockState, FileManifest, HttpAuth, HttpClientOptions, HttpConfig, HttpHeader,
    HttpRequestOptions, HttpResourceIdentity, PieceBlock, PieceLayout, PieceState, ProxyOptions,
    SessionDescriptor, SessionManifest, SessionMode, SourceCapabilities, SourceDescriptor,
    SourceKind, SourceSet,
};
pub use download::{
    DownloadSpec, Event, Manager, SegmentRequest, SegmentRequestBuilder, Session, SessionRequest,
    SessionSnapshot, StatsSnapshot, Status, Worker,
};
pub use error::Error;
pub use p2p::TorrentResumeSnapshot;
pub use p2p::{
    IndexFeedProvider, MagnetExactTopic, MagnetLink, MagnetParameter, SwarmIndexProviderConfig,
    SwarmProviderConfig, SwarmProviderLimits, TorrentDiagnosticEvent, TorrentDiagnosticScope,
    TorrentDiagnosticSeverity, TorrentPeerEndpoint, TorrentSnapshot, TorrentSwarmHints,
    TorrentSwarmProviderCandidate, TorrentSwarmProviderCandidateKind,
    TorrentSwarmProviderDiagnostic, TorrentSwarmProviderReport, TorrentSwarmProviderResolution,
    TorrentSwarmProviderSeverity, TorrentTransferStats, build_swarm_provider_resolver,
};
pub use runtime::{init_logger, init_logger_with_level};
pub use storage::{Backend, Store};
