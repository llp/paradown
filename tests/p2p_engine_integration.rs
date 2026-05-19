use async_trait::async_trait;
use paradown::p2p::{
    TorrentEngine, TorrentEngineBackend, TorrentEngineCapabilities, TorrentEngineEvent,
    TorrentEngineHandle, TorrentEngineRequest, TorrentEngineSession, TorrentEngineState,
    TorrentFileEntry, TorrentMetadata,
};
use paradown::{Backend, Config, DownloadSpec, Error, Manager};
use std::sync::Arc;

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
