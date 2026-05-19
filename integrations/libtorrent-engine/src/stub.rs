use async_trait::async_trait;
use paradown::Error;
use paradown::p2p::{
    LibtorrentEngineConfig, TorrentEngine, TorrentEngineBackend, TorrentEngineCapabilities,
    TorrentEngineHandle, TorrentEngineRequest, TorrentEngineSession,
};

#[derive(Debug)]
pub struct LibtorrentRasterbarEngine {
    config: LibtorrentEngineConfig,
}

impl LibtorrentRasterbarEngine {
    pub fn new(config: LibtorrentEngineConfig) -> Result<Self, Error> {
        Ok(Self { config })
    }

    pub fn config(&self) -> &LibtorrentEngineConfig {
        &self.config
    }
}

#[async_trait]
impl TorrentEngine for LibtorrentRasterbarEngine {
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
            "libtorrent adapter crate was built without native-libtorrent; enable that feature before starting session {} ({})",
            request.session_id,
            request.spec.locator()
        )))
    }

    async fn pause_session(&self, _handle: &TorrentEngineHandle) -> Result<(), Error> {
        Ok(())
    }

    async fn resume_session(&self, handle: &TorrentEngineHandle) -> Result<(), Error> {
        Err(Error::UnsupportedProtocol(format!(
            "libtorrent native binding is not enabled for session {}",
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
