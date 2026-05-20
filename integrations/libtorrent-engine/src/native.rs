use async_trait::async_trait;
use cxx::UniquePtr;
use paradown::Error;
use paradown::p2p::{
    LibtorrentEngineConfig, TorrentDiagnosticEvent, TorrentDiagnosticScope,
    TorrentDiagnosticSeverity, TorrentEngine, TorrentEngineBackend, TorrentEngineCapabilities,
    TorrentEngineEvent, TorrentEngineHandle, TorrentEngineRequest, TorrentEngineSession,
    TorrentEngineState, TorrentFileEntry, TorrentMetadata, TorrentPieceHash, TorrentSwarmHints,
    TorrentTracker,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

use crate::ffi::ffi::{
    NativeEngine, NativeEngineConfig, NativeEngineEvent, NativeStartResult, NativeTorrentMetadata,
};
use crate::ffi::{
    DIAGNOSTIC_SCOPE_DHT, DIAGNOSTIC_SCOPE_LISTEN, DIAGNOSTIC_SCOPE_PEER,
    DIAGNOSTIC_SCOPE_PORT_MAPPING, DIAGNOSTIC_SCOPE_SESSION, DIAGNOSTIC_SCOPE_TRACKER,
    DIAGNOSTIC_SEVERITY_ERROR, DIAGNOSTIC_SEVERITY_INFO, DIAGNOSTIC_SEVERITY_WARNING,
    EVENT_DIAGNOSTIC, EVENT_ERROR, EVENT_FINISHED, EVENT_METADATA, EVENT_PIECE_FINISHED,
    EVENT_PROGRESS, EVENT_RESUME_DATA, EVENT_STATE, STATE_CHECKING_FILES, STATE_COMPLETED,
    STATE_DOWNLOADING, STATE_PAUSED, STATE_RESOLVING_METADATA, STATE_SEEDING,
};

struct NativeDriver {
    engine: UniquePtr<NativeEngine>,
}

unsafe impl Send for NativeDriver {}

struct EngineState {
    driver: NativeDriver,
    senders: HashMap<String, mpsc::UnboundedSender<TorrentEngineEvent>>,
    polling: bool,
}

#[derive(Clone)]
pub struct LibtorrentRasterbarEngine {
    config: LibtorrentEngineConfig,
    state: Arc<Mutex<EngineState>>,
}

impl LibtorrentRasterbarEngine {
    pub fn new(config: LibtorrentEngineConfig) -> Result<Self, Error> {
        let engine = crate::ffi::ffi::new_native_engine(native_config(&config))
            .map_err(|err| Error::Other(format!("failed to create libtorrent session: {err}")))?;
        Ok(Self {
            config,
            state: Arc::new(Mutex::new(EngineState {
                driver: NativeDriver { engine },
                senders: HashMap::new(),
                polling: false,
            })),
        })
    }

    pub fn config(&self) -> &LibtorrentEngineConfig {
        &self.config
    }

    pub fn listen_port(&self) -> Result<u16, Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::listen_port(engine)
            .map_err(|err| Error::Other(format!("failed to read libtorrent listen port: {err}")))
    }

    pub fn connect_peer(
        &self,
        handle: &TorrentEngineHandle,
        host: &str,
        port: u16,
    ) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::connect_peer(engine, &handle.external_id, host, port)
            .map_err(|err| Error::Other(format!("failed to connect libtorrent peer: {err}")))
    }

    pub fn add_tracker(&self, handle: &TorrentEngineHandle, url: &str) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::add_tracker(engine, &handle.external_id, url)
            .map_err(|err| Error::Other(format!("failed to add libtorrent tracker: {err}")))
    }

    pub fn add_url_seed(&self, handle: &TorrentEngineHandle, url: &str) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::add_url_seed(engine, &handle.external_id, url)
            .map_err(|err| Error::Other(format!("failed to add libtorrent web seed: {err}")))
    }

    fn ensure_polling(&self) {
        let should_spawn = {
            let mut state = self.state.lock().expect("libtorrent state poisoned");
            if state.polling {
                false
            } else {
                state.polling = true;
                true
            }
        };

        if should_spawn {
            let state = Arc::clone(&self.state);
            tokio::spawn(async move {
                poll_libtorrent_alerts(state).await;
            });
        }
    }
}

#[async_trait]
impl TorrentEngine for LibtorrentRasterbarEngine {
    fn backend(&self) -> TorrentEngineBackend {
        TorrentEngineBackend::Libtorrent
    }

    fn capabilities(&self) -> TorrentEngineCapabilities {
        TorrentEngineCapabilities::libtorrent_full()
    }

    async fn start_session(
        &self,
        request: TorrentEngineRequest,
    ) -> Result<TorrentEngineSession, Error> {
        let resume_data = request
            .resume
            .as_ref()
            .and_then(|snapshot| snapshot.resume_data.clone());
        let resume_metadata = request
            .resume
            .as_ref()
            .and_then(|snapshot| snapshot.metadata.clone());
        let resume_bytes = resume_data.as_deref().unwrap_or(&[]);
        let save_path = request.download_dir.to_string_lossy().to_string();

        let result = {
            let mut state = self.state.lock().expect("libtorrent state poisoned");
            match &request.spec {
                paradown::DownloadSpec::Magnet { uri } => {
                    let uri = request.swarm_hints.enhance_magnet_uri(uri)?;
                    let engine = state.driver.engine.pin_mut();
                    crate::ffi::ffi::add_magnet(engine, &uri, &save_path, resume_bytes)
                }
                paradown::DownloadSpec::TorrentFile { path } => {
                    let engine = state.driver.engine.pin_mut();
                    crate::ffi::ffi::add_torrent_file(engine, path, &save_path, resume_bytes)
                }
                other => {
                    return Err(Error::UnsupportedProtocol(format!(
                        "{} is not a torrent engine spec",
                        other.scheme()
                    )));
                }
            }
        }
        .map_err(|err| Error::Other(format!("failed to add torrent: {err}")))?;

        let external_id = start_external_id(&result, request.resume.as_ref());
        apply_swarm_hints(&self.state, &external_id, &request.swarm_hints)?;
        let metadata = if result.has_metadata {
            Some(metadata_from_native(&result.metadata))
        } else {
            resume_metadata
        };

        {
            let mut state = self.state.lock().expect("libtorrent state poisoned");
            if let Some(sender) = request.event_sender {
                state.senders.insert(external_id.clone(), sender);
            }
        }
        self.ensure_polling();

        Ok(TorrentEngineSession {
            handle: TorrentEngineHandle {
                backend: TorrentEngineBackend::Libtorrent,
                external_id,
            },
            state: if metadata.is_some() {
                TorrentEngineState::Downloading
            } else {
                TorrentEngineState::ResolvingMetadata
            },
            metadata,
            manifest: None,
            resume_data,
        })
    }

    async fn pause_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::pause_torrent(engine, &handle.external_id)
            .map_err(|err| Error::Other(format!("failed to pause torrent: {err}")))
    }

    async fn resume_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::resume_torrent(engine, &handle.external_id)
            .map_err(|err| Error::Other(format!("failed to resume torrent: {err}")))
    }

    async fn cancel_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        let engine = state.driver.engine.pin_mut();
        let _ = crate::ffi::ffi::save_resume_data(engine, &handle.external_id);
        state.senders.remove(&handle.external_id);
        Ok(())
    }

    async fn remove_session(
        &self,
        handle: &TorrentEngineHandle,
        delete_payload: bool,
    ) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::remove_torrent(engine, &handle.external_id, delete_payload)
            .map_err(|err| Error::Other(format!("failed to remove torrent: {err}")))?;
        state.senders.remove(&handle.external_id);
        Ok(())
    }
}

fn apply_swarm_hints(
    state: &Arc<Mutex<EngineState>>,
    external_id: &str,
    hints: &TorrentSwarmHints,
) -> Result<(), Error> {
    if hints.is_empty() {
        return Ok(());
    }

    let mut state = state.lock().expect("libtorrent state poisoned");
    for tracker in &hints.trackers {
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::add_tracker(engine, external_id, tracker)
            .map_err(|err| Error::Other(format!("failed to add libtorrent tracker: {err}")))?;
    }
    for web_seed in &hints.web_seeds {
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::add_url_seed(engine, external_id, web_seed)
            .map_err(|err| Error::Other(format!("failed to add libtorrent web seed: {err}")))?;
    }
    for peer in &hints.peers {
        let engine = state.driver.engine.pin_mut();
        crate::ffi::ffi::connect_peer(engine, external_id, &peer.host, peer.port)
            .map_err(|err| Error::Other(format!("failed to connect libtorrent peer: {err}")))?;
    }

    Ok(())
}

async fn poll_libtorrent_alerts(state: Arc<Mutex<EngineState>>) {
    loop {
        let (dispatches, should_continue) = {
            let mut state = state.lock().expect("libtorrent state poisoned");
            let engine = state.driver.engine.pin_mut();
            match crate::ffi::ffi::poll_alerts(engine) {
                Ok(native_events) => {
                    let dispatches = native_events
                        .into_iter()
                        .flat_map(|event| translate_event(&state.senders, event))
                        .collect::<Vec<_>>();
                    if state.senders.is_empty() {
                        state.polling = false;
                        (dispatches, false)
                    } else {
                        (dispatches, true)
                    }
                }
                Err(err) => {
                    let dispatches = state
                        .senders
                        .values()
                        .cloned()
                        .map(|sender| {
                            (
                                sender,
                                TorrentEngineEvent::Error(format!(
                                    "failed to poll libtorrent alerts: {err}"
                                )),
                            )
                        })
                        .collect::<Vec<_>>();
                    state.polling = false;
                    (dispatches, false)
                }
            }
        };

        dispatch_all(dispatches).await;

        if !should_continue {
            break;
        }

        sleep(Duration::from_millis(250)).await;
    }
}

async fn dispatch_all(dispatches: Vec<(mpsc::UnboundedSender<TorrentEngineEvent>, TorrentEngineEvent)>) {
    for (sender, event) in dispatches {
        let _ = sender.send(event);
    }
}

fn translate_event(
    senders: &HashMap<String, mpsc::UnboundedSender<TorrentEngineEvent>>,
    event: NativeEngineEvent,
) -> Vec<(mpsc::UnboundedSender<TorrentEngineEvent>, TorrentEngineEvent)> {
    if event.kind == EVENT_DIAGNOSTIC && event.external_id.is_empty() {
        let diagnostic = diagnostic_from_native(&event);
        return senders
            .values()
            .cloned()
            .map(|sender| (sender, TorrentEngineEvent::Diagnostic(diagnostic.clone())))
            .collect();
    }

    let Some(sender) = senders.get(event.external_id.as_str()).cloned() else {
        return Vec::new();
    };

    match event.kind {
        EVENT_METADATA if event.has_metadata => vec![(
            sender,
            TorrentEngineEvent::MetadataDiscovered(metadata_from_native(&event.metadata)),
        )],
        EVENT_METADATA => vec![(
            sender,
            TorrentEngineEvent::StateChanged(TorrentEngineState::Downloading),
        )],
        EVENT_STATE => vec![(
            sender,
            TorrentEngineEvent::StateChanged(state_from_native(event.state)),
        )],
        EVENT_PROGRESS => vec![(
            sender,
            TorrentEngineEvent::Progress {
                downloaded: event.downloaded,
                total: event.total,
                download_rate_bps: event.download_rate_bps,
                upload_rate_bps: event.upload_rate_bps,
                connected_peers: event.connected_peers,
                seeds: event.seeds,
            },
        )],
        EVENT_PIECE_FINISHED => vec![(
            sender,
            TorrentEngineEvent::PieceFinished {
                piece_index: event.piece_index,
            },
        )],
        EVENT_RESUME_DATA => vec![(
            sender,
            TorrentEngineEvent::ResumeData {
                bytes: event.resume_data,
            },
        )],
        EVENT_DIAGNOSTIC => vec![(
            sender,
            TorrentEngineEvent::Diagnostic(diagnostic_from_native(&event)),
        )],
        EVENT_FINISHED => vec![(sender, TorrentEngineEvent::Finished)],
        EVENT_ERROR => vec![(sender, TorrentEngineEvent::Error(event.message.to_string()))],
        _ => Vec::new(),
    }
}

fn diagnostic_from_native(event: &NativeEngineEvent) -> TorrentDiagnosticEvent {
    TorrentDiagnosticEvent {
        scope: diagnostic_scope_from_native(event.diagnostic_scope),
        severity: diagnostic_severity_from_native(event.diagnostic_severity),
        message: event.message.to_string(),
        url: non_empty_string(&event.diagnostic_url),
        endpoint: non_empty_string(&event.diagnostic_endpoint),
        peers: event
            .diagnostic_has_peers
            .then_some(event.diagnostic_peers),
    }
}

fn non_empty_string(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn diagnostic_scope_from_native(scope: u8) -> TorrentDiagnosticScope {
    match scope {
        DIAGNOSTIC_SCOPE_TRACKER => TorrentDiagnosticScope::Tracker,
        DIAGNOSTIC_SCOPE_DHT => TorrentDiagnosticScope::Dht,
        DIAGNOSTIC_SCOPE_PEER => TorrentDiagnosticScope::Peer,
        DIAGNOSTIC_SCOPE_LISTEN => TorrentDiagnosticScope::Listen,
        DIAGNOSTIC_SCOPE_PORT_MAPPING => TorrentDiagnosticScope::PortMapping,
        DIAGNOSTIC_SCOPE_SESSION => TorrentDiagnosticScope::Session,
        _ => TorrentDiagnosticScope::Session,
    }
}

fn diagnostic_severity_from_native(severity: u8) -> TorrentDiagnosticSeverity {
    match severity {
        DIAGNOSTIC_SEVERITY_INFO => TorrentDiagnosticSeverity::Info,
        DIAGNOSTIC_SEVERITY_WARNING => TorrentDiagnosticSeverity::Warning,
        DIAGNOSTIC_SEVERITY_ERROR => TorrentDiagnosticSeverity::Error,
        _ => TorrentDiagnosticSeverity::Info,
    }
}

fn native_config(config: &LibtorrentEngineConfig) -> NativeEngineConfig {
    NativeEngineConfig {
        alert_queue_size: config.alert_queue_size,
        enable_dht: config.enable_dht,
        enable_lsd: config.enable_lsd,
        enable_upnp: config.enable_upnp,
        enable_natpmp: config.enable_natpmp,
        has_listen_interfaces: config.listen_interfaces.is_some(),
        listen_interfaces: config.listen_interfaces.clone().unwrap_or_default(),
    }
}

fn start_external_id(
    result: &NativeStartResult,
    resume: Option<&paradown::p2p::TorrentResumeSnapshot>,
) -> String {
    if !result.external_id.is_empty() {
        result.external_id.to_string()
    } else {
        resume
            .map(|snapshot| snapshot.handle.external_id.clone())
            .unwrap_or_else(|| "libtorrent:pending".into())
    }
}

fn metadata_from_native(metadata: &NativeTorrentMetadata) -> TorrentMetadata {
    TorrentMetadata {
        name: metadata.name.to_string(),
        info_hash_v1: metadata
            .has_info_hash_v1
            .then(|| metadata.info_hash_v1.to_string()),
        info_hash_v2: metadata
            .has_info_hash_v2
            .then(|| metadata.info_hash_v2.to_string()),
        piece_size: metadata.piece_size,
        piece_count: metadata.piece_count,
        total_size: metadata.total_size,
        private: metadata.private_torrent,
        files: metadata
            .files
            .iter()
            .map(|file| TorrentFileEntry {
                path_components: file
                    .path_components
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                length: file.length,
                offset: file.offset,
            })
            .collect(),
        piece_hashes: metadata
            .piece_hashes
            .iter()
            .map(|hash| TorrentPieceHash {
                piece_index: hash.piece_index,
                sha1: (!hash.sha1.is_empty()).then(|| hash.sha1.to_string()),
                sha256: (!hash.sha256.is_empty()).then(|| hash.sha256.to_string()),
            })
            .collect(),
        trackers: metadata
            .trackers
            .iter()
            .map(|tracker| TorrentTracker {
                url: tracker.url.to_string(),
                tier: tracker.tier,
            })
            .collect(),
        web_seeds: metadata
            .web_seeds
            .iter()
            .map(ToString::to_string)
            .collect(),
    }
}

fn state_from_native(state: u8) -> TorrentEngineState {
    match state {
        STATE_RESOLVING_METADATA => TorrentEngineState::ResolvingMetadata,
        STATE_CHECKING_FILES => TorrentEngineState::CheckingFiles,
        STATE_DOWNLOADING => TorrentEngineState::Downloading,
        STATE_SEEDING => TorrentEngineState::Seeding,
        STATE_PAUSED => TorrentEngineState::Paused,
        STATE_COMPLETED => TorrentEngineState::Completed,
        _ => TorrentEngineState::Downloading,
    }
}
