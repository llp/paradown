//! P2P 相关模块门面。
//!
//! 这里展示了一个稍复杂的模块组织模式：
//! - 多个私有子模块承载具体实现。
//! - 大量 `pub use` 负责向外重新导出稳定 API。
//! - 少量 `pub(crate) use` 只给当前 crate 内部使用。

mod engine;
mod libtorrent;
mod magnet;
mod manifest;
mod provider;

// 公开 re-export：这些类型会成为 `p2p` 模块对外 API 的一部分。
// 如果 `src/lib.rs` 再次 `pub use p2p::{...}`，它们还会被提升到 crate 根。
pub use engine::{
    TorrentDiagnosticEvent, TorrentDiagnosticScope, TorrentDiagnosticSeverity, TorrentEngine,
    TorrentEngineBackend, TorrentEngineCapabilities, TorrentEngineEvent, TorrentEngineHandle,
    TorrentEngineRequest, TorrentEngineSession, TorrentEngineState, TorrentFileEntry,
    TorrentMetadata, TorrentPieceHash, TorrentResumeSnapshot, TorrentSnapshot, TorrentTracker,
    TorrentTransferStats,
};
pub use libtorrent::{LibtorrentEngineConfig, LibtorrentEngineUnavailable};
pub use magnet::{
    MagnetExactTopic, MagnetLink, MagnetParameter, TorrentPeerEndpoint, TorrentSwarmHints,
};
pub use provider::{
    DiscoverySwarmProvider, IndexFeedProvider, MagnetHintProvider, StaticSwarmProvider,
    SwarmIndexProviderConfig, SwarmProviderConfig, SwarmProviderLimits, TorrentSwarmProvider,
    TorrentSwarmProviderCandidate, TorrentSwarmProviderCandidateKind, TorrentSwarmProviderContext,
    TorrentSwarmProviderDiagnostic, TorrentSwarmProviderReport, TorrentSwarmProviderResolution,
    TorrentSwarmProviderResolver, TorrentSwarmProviderSeverity, TrackerListProvider,
    build_swarm_provider_resolver,
};

// `pub(crate) use` 是“只在当前 crate 内重新导出”。
// 这适合内部模块共享实现细节，但不把它暴露给库使用者。
pub(crate) use libtorrent::default_libtorrent_engine;
pub(crate) use manifest::manifest_from_torrent_metadata;
