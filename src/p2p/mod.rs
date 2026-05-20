mod engine;
mod libtorrent;
mod magnet;
mod manifest;
mod provider;

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
    DiscoverySwarmProvider, MagnetHintProvider, StaticSwarmProvider, SwarmProviderConfig,
    SwarmProviderLimits, TorrentSwarmProvider, TorrentSwarmProviderCandidate,
    TorrentSwarmProviderCandidateKind, TorrentSwarmProviderContext, TorrentSwarmProviderDiagnostic,
    TorrentSwarmProviderReport, TorrentSwarmProviderResolution, TorrentSwarmProviderResolver,
    TorrentSwarmProviderSeverity, TrackerListProvider, build_swarm_provider_resolver,
};

pub(crate) use libtorrent::default_libtorrent_engine;
pub(crate) use manifest::manifest_from_torrent_metadata;
