use async_trait::async_trait;
use paradown::p2p::{
    TorrentDiagnosticEvent, TorrentDiagnosticScope, TorrentDiagnosticSeverity, TorrentEngine,
    TorrentEngineBackend, TorrentEngineCapabilities, TorrentEngineEvent, TorrentEngineHandle,
    TorrentEngineRequest, TorrentEngineSession, TorrentEngineState, TorrentFileEntry,
    TorrentMetadata, TorrentResumeSnapshot,
};
use paradown::{Backend, Config, DownloadSpec, Error, Event, Manager, Store};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct FakeTorrentEngine;

#[async_trait]
impl TorrentEngine for FakeTorrentEngine {
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
        Ok(fake_session(&request))
    }

    async fn pause_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn resume_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn cancel_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn remove_session(
        &self,
        _handle: &TorrentEngineHandle,
        _delete_payload: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Debug)]
struct FastFinishingTorrentEngine;

#[async_trait]
impl TorrentEngine for FastFinishingTorrentEngine {
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
        if let Some(sender) = request.event_sender.as_ref() {
            let _ = sender.send(TorrentEngineEvent::Finished);
        }
        Ok(fake_session(&request))
    }

    async fn pause_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn resume_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn cancel_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn remove_session(
        &self,
        _handle: &TorrentEngineHandle,
        _delete_payload: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Debug)]
struct StateChangingTorrentEngine {
    state: TorrentEngineState,
}

#[async_trait]
impl TorrentEngine for StateChangingTorrentEngine {
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
        if let Some(sender) = request.event_sender.as_ref() {
            let sender = sender.clone();
            let state = self.state.clone();
            tokio::spawn(async move {
                tokio::task::yield_now().await;
                let _ = sender.send(TorrentEngineEvent::StateChanged(state));
            });
        }
        Ok(fake_session(&request))
    }

    async fn pause_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn resume_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn cancel_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn remove_session(
        &self,
        _handle: &TorrentEngineHandle,
        _delete_payload: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Debug)]
struct ProgressReportingTorrentEngine;

#[async_trait]
impl TorrentEngine for ProgressReportingTorrentEngine {
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
        if let Some(sender) = request.event_sender.as_ref() {
            let sender = sender.clone();
            tokio::spawn(async move {
                tokio::task::yield_now().await;
                let _ = sender.send(TorrentEngineEvent::Progress {
                    downloaded: 4,
                    total: 10,
                    download_rate_bps: 2048,
                    upload_rate_bps: 512,
                    connected_peers: 7,
                    seeds: 3,
                });
            });
        }
        Ok(fake_session(&request))
    }

    async fn pause_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn resume_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn cancel_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn remove_session(
        &self,
        _handle: &TorrentEngineHandle,
        _delete_payload: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Debug)]
struct DiagnosticReportingTorrentEngine;

#[async_trait]
impl TorrentEngine for DiagnosticReportingTorrentEngine {
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
        if let Some(sender) = request.event_sender.as_ref() {
            let sender = sender.clone();
            tokio::spawn(async move {
                tokio::task::yield_now().await;
                let _ = sender.send(TorrentEngineEvent::Diagnostic(TorrentDiagnosticEvent {
                    scope: TorrentDiagnosticScope::Tracker,
                    severity: TorrentDiagnosticSeverity::Warning,
                    message: "tracker announce timed out".into(),
                    url: Some("udp://tracker.example/announce".into()),
                    endpoint: Some("127.0.0.1:6969".into()),
                    peers: None,
                }));
                let _ = sender.send(TorrentEngineEvent::Diagnostic(TorrentDiagnosticEvent {
                    scope: TorrentDiagnosticScope::Dht,
                    severity: TorrentDiagnosticSeverity::Info,
                    message: "DHT lookup returned peers".into(),
                    url: None,
                    endpoint: None,
                    peers: Some(5),
                }));
            });
        }
        Ok(fake_session(&request))
    }

    async fn pause_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn resume_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn cancel_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn remove_session(
        &self,
        _handle: &TorrentEngineHandle,
        _delete_payload: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Debug)]
struct ResumeRecordingTorrentEngine {
    received_resume: Arc<Mutex<Option<TorrentResumeSnapshot>>>,
    emit_resume_data: Option<Vec<u8>>,
}

#[async_trait]
impl TorrentEngine for ResumeRecordingTorrentEngine {
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
        *self.received_resume.lock().unwrap() = request.resume.clone();
        if let Some(bytes) = self.emit_resume_data.clone()
            && let Some(sender) = request.event_sender.as_ref()
        {
            let sender = sender.clone();
            tokio::spawn(async move {
                tokio::task::yield_now().await;
                let _ = sender.send(TorrentEngineEvent::ResumeData { bytes });
            });
        }
        Ok(fake_session(&request))
    }

    async fn pause_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn resume_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn cancel_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn remove_session(
        &self,
        _handle: &TorrentEngineHandle,
        _delete_payload: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
}

fn fake_session(request: &TorrentEngineRequest) -> TorrentEngineSession {
    TorrentEngineSession {
        handle: TorrentEngineHandle {
            backend: TorrentEngineBackend::Libtorrent,
            external_id: format!("lt-{}", request.session_id),
        },
        state: TorrentEngineState::Downloading,
        metadata: Some(TorrentMetadata {
            name: "payload".into(),
            info_hash_v1: Some("0123456789abcdef0123456789abcdef01234567".into()),
            info_hash_v2: None,
            piece_size: 4,
            piece_count: 3,
            total_size: 10,
            private: false,
            files: vec![TorrentFileEntry {
                path_components: vec!["payload.bin".into()],
                length: 10,
                offset: 0,
            }],
            piece_hashes: Vec::new(),
            trackers: Vec::new(),
            web_seeds: Vec::new(),
        }),
        manifest: None,
        resume_data: request
            .resume
            .as_ref()
            .and_then(|resume| resume.resume_data.clone()),
    }
}

fn p2p_config(sandbox: &tempfile::TempDir) -> Config {
    let mut config = Config::default();
    config.download_dir = sandbox.path().join("downloads");
    config.storage_backend = Backend::Sqlite(sandbox.path().join("downloads.db"));
    config
}

async fn add_magnet(manager: &Arc<Manager>) -> u32 {
    manager
        .add_download(
            DownloadSpec::parse(
                "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=payload",
            )
            .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn torrent_sessions_use_injected_engine_and_manifest_mapping() {
    let sandbox = tempfile::TempDir::new().unwrap();
    let manager =
        Manager::new_with_torrent_engine(p2p_config(&sandbox), Arc::new(FakeTorrentEngine))
            .unwrap();
    manager.init().await.unwrap();

    let task_id = add_magnet(&manager).await;
    manager.start_task(task_id).await.unwrap();

    let snapshot = manager.get_session(task_id).unwrap().snapshot().await;
    assert_eq!(snapshot.status, "Running");
    assert_eq!(snapshot.total_size, 10);
    assert_eq!(snapshot.piece_count, 3);
    assert_eq!(snapshot.block_count, 3);
    let torrent = snapshot.torrent.expect("torrent snapshot");
    assert_eq!(torrent.backend, TorrentEngineBackend::Libtorrent);
    assert_eq!(torrent.external_id, format!("lt-{task_id}"));
    assert_eq!(
        torrent.info_hash_v1.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    assert_eq!(torrent.name.as_deref(), Some("payload"));
    assert_eq!(torrent.piece_count, Some(3));
    assert_eq!(torrent.file_count, Some(1));
    assert_eq!(
        snapshot.file_path.as_deref(),
        Some(sandbox.path().join("downloads/payload.bin").as_path())
    );
}

#[tokio::test]
async fn torrent_engine_finish_event_cannot_be_overwritten_by_start_transition() {
    let sandbox = tempfile::TempDir::new().unwrap();
    let manager = Manager::new_with_torrent_engine(
        p2p_config(&sandbox),
        Arc::new(FastFinishingTorrentEngine),
    )
    .unwrap();
    manager.init().await.unwrap();

    let task_id = add_magnet(&manager).await;
    manager.start_task(task_id).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let snapshot = manager.get_session(task_id).unwrap().snapshot().await;
    assert_eq!(snapshot.status, "Completed");
    assert_eq!(snapshot.completed_pieces, 3);
}

#[tokio::test]
async fn torrent_engine_seeding_state_completes_task() {
    let sandbox = tempfile::TempDir::new().unwrap();
    let manager = Manager::new_with_torrent_engine(
        p2p_config(&sandbox),
        Arc::new(StateChangingTorrentEngine {
            state: TorrentEngineState::Seeding,
        }),
    )
    .unwrap();
    manager.init().await.unwrap();

    let task_id = add_magnet(&manager).await;
    manager.start_task(task_id).await.unwrap();

    let mut snapshot = manager.get_session(task_id).unwrap().snapshot().await;
    for _ in 0..80 {
        if snapshot.status == "Completed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        snapshot = manager.get_session(task_id).unwrap().snapshot().await;
    }

    assert_eq!(snapshot.status, "Completed");
    assert_eq!(snapshot.completed_pieces, 3);
}

#[tokio::test]
async fn torrent_progress_updates_public_swarm_snapshot() {
    let sandbox = tempfile::TempDir::new().unwrap();
    let manager = Manager::new_with_torrent_engine(
        p2p_config(&sandbox),
        Arc::new(ProgressReportingTorrentEngine),
    )
    .unwrap();
    manager.init().await.unwrap();

    let task_id = add_magnet(&manager).await;
    manager.start_task(task_id).await.unwrap();

    let mut snapshot = manager.get_session(task_id).unwrap().snapshot().await;
    for _ in 0..80 {
        if snapshot
            .torrent
            .as_ref()
            .is_some_and(|torrent| torrent.connected_peers == 7)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        snapshot = manager.get_session(task_id).unwrap().snapshot().await;
    }

    let torrent = snapshot.torrent.expect("torrent snapshot");
    assert_eq!(snapshot.downloaded_size, 4);
    assert_eq!(snapshot.total_size, 10);
    assert_eq!(torrent.downloaded, 4);
    assert_eq!(torrent.total, 10);
    assert_eq!(torrent.download_rate_bps, 2048);
    assert_eq!(torrent.upload_rate_bps, 512);
    assert_eq!(torrent.connected_peers, 7);
    assert_eq!(torrent.seeds, 3);
}

#[tokio::test]
async fn torrent_diagnostics_update_snapshot_and_event_stream() {
    let sandbox = tempfile::TempDir::new().unwrap();
    let manager = Manager::new_with_torrent_engine(
        p2p_config(&sandbox),
        Arc::new(DiagnosticReportingTorrentEngine),
    )
    .unwrap();
    manager.init().await.unwrap();
    let mut events = manager.subscribe_events();

    let task_id = add_magnet(&manager).await;
    manager.start_task(task_id).await.unwrap();

    let mut snapshot = manager.get_session(task_id).unwrap().snapshot().await;
    for _ in 0..80 {
        if snapshot
            .torrent
            .as_ref()
            .is_some_and(|torrent| torrent.diagnostics.len() >= 2)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        snapshot = manager.get_session(task_id).unwrap().snapshot().await;
    }

    let torrent = snapshot.torrent.expect("torrent snapshot");
    assert_eq!(torrent.diagnostics.len(), 2);
    assert_eq!(
        torrent.diagnostics[0].scope,
        TorrentDiagnosticScope::Tracker
    );
    assert_eq!(
        torrent.diagnostics[0].url.as_deref(),
        Some("udp://tracker.example/announce")
    );
    assert_eq!(torrent.diagnostics[1].scope, TorrentDiagnosticScope::Dht);
    assert_eq!(torrent.diagnostics[1].peers, Some(5));

    let mut saw_diagnostic_event = false;
    for _ in 0..16 {
        match tokio::time::timeout(std::time::Duration::from_millis(50), events.recv()).await {
            Ok(Ok(Event::TorrentDiagnostic { id, diagnostic })) if id == task_id => {
                saw_diagnostic_event = diagnostic.scope == TorrentDiagnosticScope::Tracker
                    || diagnostic.scope == TorrentDiagnosticScope::Dht;
                if saw_diagnostic_event {
                    break;
                }
            }
            Ok(Ok(_)) => {}
            _ => {}
        }
    }
    assert!(saw_diagnostic_event);
}

#[tokio::test]
async fn torrent_resume_data_is_persisted_and_reused_after_restore() {
    let sandbox = tempfile::TempDir::new().unwrap();
    let config = p2p_config(&sandbox);
    let emitted_resume = vec![1, 2, 3, 5, 8, 13];
    let first_engine = Arc::new(ResumeRecordingTorrentEngine {
        received_resume: Arc::new(Mutex::new(None)),
        emit_resume_data: Some(emitted_resume.clone()),
    });
    let manager = Manager::new_with_torrent_engine(config.clone(), first_engine).unwrap();
    manager.init().await.unwrap();

    let task_id = add_magnet(&manager).await;
    manager.start_task(task_id).await.unwrap();

    let store = Store::new(Arc::new(config.clone())).await.unwrap();
    let mut persisted = store.load_task(task_id).await.unwrap().unwrap();
    for _ in 0..80 {
        if persisted.torrent_backend.as_deref() == Some("libtorrent")
            && persisted.torrent_resume_data.as_deref() == Some(emitted_resume.as_slice())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        persisted = store.load_task(task_id).await.unwrap().unwrap();
    }
    assert_eq!(persisted.torrent_backend.as_deref(), Some("libtorrent"));
    assert_eq!(
        persisted.torrent_resume_data.as_deref(),
        Some(emitted_resume.as_slice())
    );
    assert!(
        persisted
            .torrent_metadata_json
            .as_deref()
            .is_some_and(|json| json.contains("payload"))
    );

    let received_resume = Arc::new(Mutex::new(None));
    let restore_engine = Arc::new(ResumeRecordingTorrentEngine {
        received_resume: Arc::clone(&received_resume),
        emit_resume_data: None,
    });
    let restored_manager = Manager::new_with_torrent_engine(config, restore_engine).unwrap();
    restored_manager.init().await.unwrap();
    restored_manager.start_task(task_id).await.unwrap();

    let resume = received_resume.lock().unwrap().clone().unwrap();
    assert_eq!(resume.handle.backend, TorrentEngineBackend::Libtorrent);
    assert_eq!(
        resume.resume_data.as_deref(),
        Some(emitted_resume.as_slice())
    );
    assert_eq!(
        resume.metadata.as_ref().map(|metadata| metadata.total_size),
        Some(10)
    );
}
