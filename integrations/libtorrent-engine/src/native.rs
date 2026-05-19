use async_trait::async_trait;
use lt_rs::add_torrent_params::AddTorrentParams;
use lt_rs::alerts::{Alert, AlertCategory, TorrentState};
use lt_rs::session::LtSession;
use lt_rs::settings_pack::SettingsPack;
use lt_rs::torrent_handle::{ResumeDataFlags, StatusFlags, TorrentHandle};
use paradown::Error;
use paradown::p2p::{
    LibtorrentEngineConfig, TorrentEngine, TorrentEngineBackend, TorrentEngineCapabilities,
    TorrentEngineEvent, TorrentEngineHandle, TorrentEngineRequest, TorrentEngineSession,
    TorrentEngineState,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

struct EngineState {
    session: LtSession,
    senders: HashMap<String, mpsc::UnboundedSender<TorrentEngineEvent>>,
    sessions: HashMap<String, NativeTorrentSession>,
    polling: bool,
}

struct NativeTorrentSession {
    handle: Option<TorrentHandle>,
    state: TorrentEngineState,
    metadata_total_size: Option<u64>,
    resume_data: Option<Vec<u8>>,
}

#[derive(Clone)]
pub struct LibtorrentRasterbarEngine {
    config: LibtorrentEngineConfig,
    state: Arc<Mutex<EngineState>>,
}

impl LibtorrentRasterbarEngine {
    pub fn new(config: LibtorrentEngineConfig) -> Result<Self, Error> {
        let mut settings = SettingsPack::new();
        settings.set_alert_mask(
            AlertCategory::Error
                | AlertCategory::Status
                | AlertCategory::Storage
                | AlertCategory::Tracker
                | AlertCategory::Dht
                | AlertCategory::PieceProgress
                | AlertCategory::FileProgress,
        );

        let session = LtSession::new_with_settings(&settings);
        Ok(Self {
            config,
            state: Arc::new(Mutex::new(EngineState {
                session,
                senders: HashMap::new(),
                sessions: HashMap::new(),
                polling: false,
            })),
        })
    }

    pub fn config(&self) -> &LibtorrentEngineConfig {
        &self.config
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
        let mut params = if let Some(bytes) = resume_data.as_deref() {
            AddTorrentParams::load_resume_data(bytes)
        } else {
            match &request.spec {
                paradown::DownloadSpec::Magnet { uri } => AddTorrentParams::parse_magnet_uri(uri),
                paradown::DownloadSpec::TorrentFile { path } => {
                    return Err(Error::UnsupportedProtocol(format!(
                        "native lt-rs torrent-file loading is not exposed yet; extend the CXX adapter before adding {path}"
                    )));
                }
                other => {
                    return Err(Error::UnsupportedProtocol(format!(
                        "{} is not a torrent engine spec",
                        other.scheme()
                    )));
                }
            }
        };
        params.set_path(&request.download_dir.to_string_lossy());
        let external_id = request
            .resume
            .as_ref()
            .map(|snapshot| snapshot.handle.external_id.clone())
            .unwrap_or_else(|| params.get_info_hash().as_base64());

        {
            let mut state = self.state.lock().expect("libtorrent state poisoned");
            if let Some(sender) = request.event_sender {
                state.senders.insert(external_id.clone(), sender);
            }
            state.sessions.insert(
                external_id.clone(),
                NativeTorrentSession {
                    handle: None,
                    state: TorrentEngineState::ResolvingMetadata,
                    metadata_total_size: resume_metadata
                        .as_ref()
                        .map(|metadata| metadata.total_size),
                    resume_data: resume_data.clone(),
                },
            );
            state.session.async_add_torrent(&params);
        }
        self.ensure_polling();

        Ok(TorrentEngineSession {
            handle: TorrentEngineHandle {
                backend: TorrentEngineBackend::Libtorrent,
                external_id,
            },
            state: TorrentEngineState::ResolvingMetadata,
            metadata: resume_metadata,
            manifest: None,
            resume_data,
        })
    }

    async fn pause_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        if let Some(session) = state.sessions.get_mut(&handle.external_id) {
            session.state = TorrentEngineState::Paused;
            if let Some(handle) = session.handle.as_ref() {
                handle.save_resume_data(ResumeDataFlags::SaveInfoDict);
            }
        }
        Ok(())
    }

    async fn resume_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        if let Some(session) = state.sessions.get_mut(&handle.external_id) {
            session.state = TorrentEngineState::Downloading;
        }
        Ok(())
    }

    async fn cancel_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        if let Some(session) = state.sessions.get(&handle.external_id)
            && let Some(handle) = session.handle.as_ref()
        {
            handle.save_resume_data(ResumeDataFlags::SaveInfoDict);
        }
        state.senders.remove(&handle.external_id);
        state.sessions.remove(&handle.external_id);
        Ok(())
    }

    async fn remove_session(
        &self,
        handle: &TorrentEngineHandle,
        _delete_payload: bool,
    ) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        if let Some(session) = state.sessions.get(&handle.external_id)
            && let Some(handle) = session.handle.as_ref()
        {
            handle.save_resume_data(ResumeDataFlags::SaveInfoDict);
        }
        state.senders.remove(&handle.external_id);
        state.sessions.remove(&handle.external_id);
        Ok(())
    }
}

async fn poll_libtorrent_alerts(state: Arc<Mutex<EngineState>>) {
    loop {
        let (dispatches, should_continue) = {
            let mut state = state.lock().expect("libtorrent state poisoned");
            state
                .session
                .post_torrent_updates(StatusFlags::QueryAccurateDownloadCounters);
            state.session.pop_alerts();
            let alerts = unsafe { state.session.take_alerts() };
            let dispatches = alerts
                .into_iter()
                .flat_map(|alert| translate_alert(&mut state, alert))
                .collect::<Vec<_>>();
            if state.senders.is_empty() && state.sessions.is_empty() {
                state.polling = false;
                (dispatches, false)
            } else {
                (dispatches, true)
            }
        };

        for (sender, event) in dispatches {
            let _ = sender.send(event);
        }

        if !should_continue {
            break;
        }

        sleep(Duration::from_millis(250)).await;
    }
}

fn translate_alert(
    state: &mut EngineState,
    alert: Alert,
) -> Vec<(mpsc::UnboundedSender<TorrentEngineEvent>, TorrentEngineEvent)> {
    match alert {
        Alert::AddTorrent(alert) => {
            let handle = alert.handle();
            let external_id = handle.info_hashes().as_base64();
            if !alert.error().is_ok() {
                return dispatch_for_external_id(
                    state,
                    &external_id,
                    TorrentEngineEvent::Error(alert.error().to_string()),
                )
                .into_iter()
                .collect();
            }
            state
                .sessions
                .entry(external_id.clone())
                .or_insert_with(|| NativeTorrentSession {
                    handle: None,
                    state: TorrentEngineState::ResolvingMetadata,
                    metadata_total_size: None,
                    resume_data: None,
                })
                .handle = Some(handle);
            dispatch_for_external_id(
                state,
                &external_id,
                TorrentEngineEvent::StateChanged(TorrentEngineState::ResolvingMetadata),
            )
            .into_iter()
            .collect()
        }
        Alert::MetadataReceived(alert) => {
            let external_id = alert.handle().info_hashes().as_base64();
            if let Some(session) = state.sessions.get_mut(&external_id) {
                session.state = TorrentEngineState::Downloading;
            }
            dispatch_for_external_id(
                state,
                &external_id,
                TorrentEngineEvent::StateChanged(TorrentEngineState::Downloading),
            )
            .into_iter()
            .collect()
        }
        Alert::PieceFinished(alert) => {
            let external_id = alert.handle().info_hashes().as_base64();
            dispatch_for_external_id(
                state,
                &external_id,
                TorrentEngineEvent::PieceFinished {
                    piece_index: alert.piece_index() as u32,
                },
            )
            .into_iter()
            .collect()
        }
        Alert::TorrentFinished(alert) => {
            let external_id = alert.handle().info_hashes().as_base64();
            if let Some(session) = state.sessions.get_mut(&external_id) {
                session.state = TorrentEngineState::Completed;
            }
            dispatch_for_external_id(state, &external_id, TorrentEngineEvent::Finished)
                .into_iter()
                .collect()
        }
        Alert::TorrentError(alert) => {
            let external_id = alert.handle().info_hashes().as_base64();
            dispatch_for_external_id(state, &external_id, TorrentEngineEvent::Error(alert.message()))
                .into_iter()
                .collect()
        }
        Alert::SaveResumeData(alert) => {
            let external_id = alert.handle().info_hashes().as_base64();
            let bytes = alert.params().write_resume_data_buf();
            if let Some(session) = state.sessions.get_mut(&external_id) {
                session.resume_data = Some(bytes.clone());
            }
            dispatch_for_external_id(
                state,
                &external_id,
                TorrentEngineEvent::ResumeData { bytes },
            )
            .into_iter()
            .collect()
        }
        Alert::SaveResumeDataFailed(alert) => {
            let external_id = alert.handle().info_hashes().as_base64();
            dispatch_for_external_id(state, &external_id, TorrentEngineEvent::Error(alert.message()))
                .into_iter()
                .collect()
        }
        Alert::StateUpdate(alert) => {
            let mut dispatches = Vec::new();
            for status in alert.status().iter() {
                let external_id = status.handle().info_hashes().as_base64();
                let engine_state = state_from_libtorrent(status.state());
                let total = state
                    .sessions
                    .get(&external_id)
                    .and_then(|session| session.metadata_total_size)
                    .unwrap_or_default();
                let downloaded = ((total as f64) * status.progress()).round() as u64;
                if let Some(session) = state.sessions.get_mut(&external_id) {
                    session.state = engine_state.clone();
                }
                if let Some(dispatch) = dispatch_for_external_id(
                    state,
                    &external_id,
                    TorrentEngineEvent::StateChanged(engine_state),
                ) {
                    dispatches.push(dispatch);
                }
                if total > 0
                    && let Some(dispatch) = dispatch_for_external_id(
                        state,
                        &external_id,
                        TorrentEngineEvent::Progress {
                            downloaded,
                            total,
                            download_rate_bps: 0,
                            upload_rate_bps: 0,
                            connected_peers: 0,
                            seeds: 0,
                        },
                    )
                {
                    dispatches.push(dispatch);
                }
            }
            dispatches
        }
        _ => Vec::new(),
    }
}

fn dispatch_for_external_id(
    state: &EngineState,
    external_id: &str,
    event: TorrentEngineEvent,
) -> Option<(mpsc::UnboundedSender<TorrentEngineEvent>, TorrentEngineEvent)> {
    state
        .senders
        .get(external_id)
        .cloned()
        .map(|sender| (sender, event))
}

fn state_from_libtorrent(state: TorrentState) -> TorrentEngineState {
    match state {
        TorrentState::CheckingFiles | TorrentState::CheckingResumeData => {
            TorrentEngineState::CheckingFiles
        }
        TorrentState::DownloadingMetadata => TorrentEngineState::ResolvingMetadata,
        TorrentState::Downloading => TorrentEngineState::Downloading,
        TorrentState::Finished => TorrentEngineState::Completed,
        TorrentState::Seeding => TorrentEngineState::Seeding,
        #[allow(unreachable_patterns)]
        _ => TorrentEngineState::Downloading,
    }
}
