#[cxx::bridge(namespace = "paradown_libtorrent")]
pub(crate) mod ffi {
    #[derive(Debug, Clone)]
    pub struct NativeEngineConfig {
        pub alert_queue_size: usize,
        pub enable_dht: bool,
        pub enable_lsd: bool,
        pub enable_upnp: bool,
        pub enable_natpmp: bool,
        pub has_listen_interfaces: bool,
        pub listen_interfaces: String,
    }

    #[derive(Debug, Clone)]
    pub struct NativeTorrentFileEntry {
        pub path_components: Vec<String>,
        pub length: u64,
        pub offset: u64,
    }

    #[derive(Debug, Clone)]
    pub struct NativeTorrentPieceHash {
        pub piece_index: u32,
        pub sha1: String,
        pub sha256: String,
    }

    #[derive(Debug, Clone)]
    pub struct NativeTorrentTracker {
        pub url: String,
        pub tier: u32,
    }

    #[derive(Debug, Clone)]
    pub struct NativeTorrentMetadata {
        pub name: String,
        pub has_info_hash_v1: bool,
        pub info_hash_v1: String,
        pub has_info_hash_v2: bool,
        pub info_hash_v2: String,
        pub piece_size: u32,
        pub piece_count: u32,
        pub total_size: u64,
        pub private_torrent: bool,
        pub files: Vec<NativeTorrentFileEntry>,
        pub piece_hashes: Vec<NativeTorrentPieceHash>,
        pub trackers: Vec<NativeTorrentTracker>,
        pub web_seeds: Vec<String>,
    }

    #[derive(Debug, Clone)]
    pub struct NativeStartResult {
        pub external_id: String,
        pub has_metadata: bool,
        pub metadata: NativeTorrentMetadata,
    }

    #[derive(Debug, Clone)]
    pub struct NativeEngineEvent {
        pub kind: u8,
        pub external_id: String,
        pub state: u8,
        pub message: String,
        pub piece_index: u32,
        pub downloaded: u64,
        pub total: u64,
        pub download_rate_bps: u64,
        pub upload_rate_bps: u64,
        pub connected_peers: u32,
        pub seeds: u32,
        pub resume_data: Vec<u8>,
        pub has_metadata: bool,
        pub metadata: NativeTorrentMetadata,
        pub diagnostic_scope: u8,
        pub diagnostic_severity: u8,
        pub diagnostic_url: String,
        pub diagnostic_endpoint: String,
        pub diagnostic_has_peers: bool,
        pub diagnostic_peers: u32,
    }

    unsafe extern "C++" {
        include!("paradown_libtorrent/native_engine.hpp");

        type NativeEngine;

        fn new_native_engine(config: NativeEngineConfig) -> Result<UniquePtr<NativeEngine>>;

        fn add_magnet(
            engine: Pin<&mut NativeEngine>,
            uri: &str,
            save_path: &str,
            resume_data: &[u8],
        ) -> Result<NativeStartResult>;

        fn add_torrent_file(
            engine: Pin<&mut NativeEngine>,
            path: &str,
            save_path: &str,
            resume_data: &[u8],
        ) -> Result<NativeStartResult>;

        fn poll_alerts(engine: Pin<&mut NativeEngine>) -> Result<Vec<NativeEngineEvent>>;

        fn listen_port(engine: Pin<&mut NativeEngine>) -> Result<u16>;
        fn connect_peer(
            engine: Pin<&mut NativeEngine>,
            external_id: &str,
            host: &str,
            port: u16,
        ) -> Result<()>;
        fn add_tracker(engine: Pin<&mut NativeEngine>, external_id: &str, url: &str)
        -> Result<()>;
        fn add_url_seed(
            engine: Pin<&mut NativeEngine>,
            external_id: &str,
            url: &str,
        ) -> Result<()>;
        fn pause_torrent(engine: Pin<&mut NativeEngine>, external_id: &str) -> Result<()>;
        fn resume_torrent(engine: Pin<&mut NativeEngine>, external_id: &str) -> Result<()>;
        fn remove_torrent(
            engine: Pin<&mut NativeEngine>,
            external_id: &str,
            delete_payload: bool,
        ) -> Result<()>;
        fn save_resume_data(engine: Pin<&mut NativeEngine>, external_id: &str) -> Result<()>;
    }
}

pub(crate) const EVENT_METADATA: u8 = 1;
pub(crate) const EVENT_STATE: u8 = 2;
pub(crate) const EVENT_PROGRESS: u8 = 3;
pub(crate) const EVENT_PIECE_FINISHED: u8 = 4;
pub(crate) const EVENT_RESUME_DATA: u8 = 5;
pub(crate) const EVENT_FINISHED: u8 = 6;
pub(crate) const EVENT_ERROR: u8 = 7;
pub(crate) const EVENT_DIAGNOSTIC: u8 = 8;

pub(crate) const STATE_RESOLVING_METADATA: u8 = 1;
pub(crate) const STATE_CHECKING_FILES: u8 = 2;
pub(crate) const STATE_DOWNLOADING: u8 = 3;
pub(crate) const STATE_SEEDING: u8 = 4;
pub(crate) const STATE_PAUSED: u8 = 5;
pub(crate) const STATE_COMPLETED: u8 = 6;

pub(crate) const DIAGNOSTIC_SCOPE_TRACKER: u8 = 1;
pub(crate) const DIAGNOSTIC_SCOPE_DHT: u8 = 2;
pub(crate) const DIAGNOSTIC_SCOPE_PEER: u8 = 3;
pub(crate) const DIAGNOSTIC_SCOPE_LISTEN: u8 = 4;
pub(crate) const DIAGNOSTIC_SCOPE_PORT_MAPPING: u8 = 5;
pub(crate) const DIAGNOSTIC_SCOPE_SESSION: u8 = 6;

pub(crate) const DIAGNOSTIC_SEVERITY_INFO: u8 = 1;
pub(crate) const DIAGNOSTIC_SEVERITY_WARNING: u8 = 2;
pub(crate) const DIAGNOSTIC_SEVERITY_ERROR: u8 = 3;
