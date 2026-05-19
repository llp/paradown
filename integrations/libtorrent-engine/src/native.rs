use async_trait::async_trait;
use lt_rs::add_torrent_params::AddTorrentParams;
use lt_rs::alerts::{Alert, AlertCategory};
use lt_rs::session::LtSession;
use lt_rs::settings_pack::SettingsPack;
use lt_rs::torrent_handle::StatusFlags;
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
    polling: bool,
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
        let mut params = match &request.spec {
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
        };
        params.set_path(&request.download_dir.to_string_lossy());
        let external_id = params.get_info_hash().as_base64();

        {
            let mut state = self.state.lock().expect("libtorrent state poisoned");
            if let Some(sender) = request.event_sender {
                state.senders.insert(external_id.clone(), sender);
            }
            state.session.async_add_torrent(&params);
        }
        self.ensure_polling();

        Ok(TorrentEngineSession {
            handle: TorrentEngineHandle {
                backend: TorrentEngineBackend::Libtorrent,
                external_id,
            },
            state: TorrentEngineState::ResolvingMetadata,
            metadata: None,
            manifest: None,
        })
    }

    async fn pause_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn resume_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn cancel_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        state.senders.remove(&handle.external_id);
        Ok(())
    }

    async fn remove_session(
        &self,
        handle: &TorrentEngineHandle,
        _delete_payload: bool,
    ) -> Result<(), Error> {
        let mut state = self.state.lock().expect("libtorrent state poisoned");
        state.senders.remove(&handle.external_id);
        Ok(())
    }
}

async fn poll_libtorrent_alerts(state: Arc<Mutex<EngineState>>) {
    loop {
        let dispatches = {
            let mut state = state.lock().expect("libtorrent state poisoned");
            state
                .session
                .post_torrent_updates(StatusFlags::QueryAccurateDownloadCounters);
            state.session.pop_alerts();
            let alerts = unsafe { state.session.take_alerts() };
            alerts
                .into_iter()
                .filter_map(|alert| translate_alert(&state.senders, alert))
                .collect::<Vec<_>>()
        };

        for (sender, event) in dispatches {
            let _ = sender.send(event);
        }

        sleep(Duration::from_millis(250)).await;
    }
}

fn translate_alert(
    senders: &HashMap<String, mpsc::UnboundedSender<TorrentEngineEvent>>,
    alert: Alert,
) -> Option<(mpsc::UnboundedSender<TorrentEngineEvent>, TorrentEngineEvent)> {
    match alert {
        Alert::MetadataReceived(alert) => {
            let sender = sender_for_handle(senders, alert.handle().info_hashes().as_base64())?;
            Some((sender, TorrentEngineEvent::StateChanged(TorrentEngineState::Downloading)))
        }
        Alert::PieceFinished(alert) => {
            let sender = sender_for_handle(senders, alert.handle().info_hashes().as_base64())?;
            Some((
                sender,
                TorrentEngineEvent::PieceFinished {
                    piece_index: alert.piece_index() as u32,
                },
            ))
        }
        Alert::TorrentFinished(alert) => {
            let sender = sender_for_handle(senders, alert.handle().info_hashes().as_base64())?;
            Some((sender, TorrentEngineEvent::Finished))
        }
        Alert::TorrentError(alert) => {
            let sender = sender_for_handle(senders, alert.handle().info_hashes().as_base64())?;
            Some((sender, TorrentEngineEvent::Error(alert.message())))
        }
        Alert::SaveResumeData(alert) => {
            let sender = sender_for_handle(senders, alert.handle().info_hashes().as_base64())?;
            Some((
                sender,
                TorrentEngineEvent::ResumeData {
                    bytes: alert.params().write_resume_data_buf(),
                },
            ))
        }
        Alert::SaveResumeDataFailed(alert) => {
            let sender = sender_for_handle(senders, alert.handle().info_hashes().as_base64())?;
            Some((sender, TorrentEngineEvent::Error(alert.message())))
        }
        _ => None,
    }
}

fn sender_for_handle(
    senders: &HashMap<String, mpsc::UnboundedSender<TorrentEngineEvent>>,
    external_id: String,
) -> Option<mpsc::UnboundedSender<TorrentEngineEvent>> {
    senders.get(&external_id).cloned()
}
