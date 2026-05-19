use super::{
    TorrentEngine, TorrentEngineBackend, TorrentEngineCapabilities, TorrentEngineHandle,
    TorrentEngineRequest, TorrentEngineSession,
};
use crate::error::Error;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibtorrentEngineConfig {
    #[serde(default = "default_alert_queue_size")]
    pub alert_queue_size: usize,
    #[serde(default = "default_enabled")]
    pub enable_dht: bool,
    #[serde(default = "default_enabled")]
    pub enable_lsd: bool,
    #[serde(default = "default_enabled")]
    pub enable_upnp: bool,
    #[serde(default = "default_enabled")]
    pub enable_natpmp: bool,
    #[serde(default)]
    pub listen_interfaces: Option<String>,
}

impl Default for LibtorrentEngineConfig {
    fn default() -> Self {
        Self {
            alert_queue_size: 1024,
            enable_dht: true,
            enable_lsd: true,
            enable_upnp: true,
            enable_natpmp: true,
            listen_interfaces: None,
        }
    }
}

fn default_alert_queue_size() -> usize {
    1024
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Default)]
pub struct LibtorrentEngineUnavailable {
    pub config: LibtorrentEngineConfig,
}

impl LibtorrentEngineUnavailable {
    pub fn new(config: LibtorrentEngineConfig) -> Self {
        Self { config }
    }
}

pub(crate) fn default_libtorrent_engine(config: LibtorrentEngineConfig) -> Arc<dyn TorrentEngine> {
    Arc::new(LibtorrentEngineUnavailable::new(config))
}

#[async_trait]
impl TorrentEngine for LibtorrentEngineUnavailable {
    fn backend(&self) -> TorrentEngineBackend {
        TorrentEngineBackend::Libtorrent
    }

    fn capabilities(&self) -> TorrentEngineCapabilities {
        TorrentEngineCapabilities::libtorrent_unavailable()
    }

    async fn start_session(
        &self,
        request: TorrentEngineRequest,
    ) -> Result<TorrentEngineSession, Error> {
        Err(Error::UnsupportedProtocol(format!(
            "{} requires a libtorrent engine adapter; build or inject the paradown libtorrent adapter before starting session {}",
            request.spec.scheme(),
            request.session_id
        )))
    }

    async fn pause_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn resume_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error> {
        Err(Error::UnsupportedProtocol(format!(
            "libtorrent engine adapter is not configured for session {}",
            handle.external_id
        )))
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
