#![cfg(feature = "native-libtorrent")]

use paradown::download::{
    DownloadSpec, LibtorrentEngineConfig, TorrentEngine, TorrentEngineBackend, TorrentEngineEvent,
};
use paradown_libtorrent_engine::LibtorrentRasterbarEngine;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

#[test]
fn native_bridge_extracts_torrent_file_metadata() {
    let sandbox = unique_sandbox();
    fs::create_dir_all(&sandbox).unwrap();
    let torrent_path = sandbox.join("sample.torrent");
    let download_dir = sandbox.join("downloads");
    fs::create_dir_all(&download_dir).unwrap();
    fs::write(&torrent_path, single_file_torrent()).unwrap();

    let engine = LibtorrentRasterbarEngine::new(LibtorrentEngineConfig {
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        listen_interfaces: Some("127.0.0.1:0".into()),
        ..LibtorrentEngineConfig::default()
    })
    .unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    let session = runtime
        .block_on(engine.start_session(paradown::p2p::TorrentEngineRequest {
            session_id: 1,
            spec: DownloadSpec::TorrentFile {
                path: torrent_path.to_string_lossy().into_owned(),
            },
            download_dir,
            requested_file_name: None,
            requested_file_path: None,
            rate_limit_kib_per_sec: None,
            resume: None,
            event_sender: None,
        }))
        .unwrap();

    assert_eq!(session.handle.backend, TorrentEngineBackend::Libtorrent);
    assert_eq!(session.handle.external_id.len(), 40);

    let metadata = session.metadata.expect("torrent metadata");
    assert_eq!(metadata.name, "hello.txt");
    assert_eq!(metadata.total_size, 5);
    assert_eq!(metadata.piece_size, 16_384);
    assert_eq!(metadata.piece_count, 1);
    assert_eq!(metadata.info_hash_v1.as_ref().map(String::len), Some(40));
    assert_eq!(metadata.files.len(), 1);
    assert_eq!(metadata.files[0].path_components, ["hello.txt"]);
    assert_eq!(metadata.files[0].length, 5);
    assert_eq!(metadata.files[0].offset, 0);
    assert_eq!(metadata.piece_hashes.len(), 1);
    assert_eq!(metadata.piece_hashes[0].piece_index, 0);
    assert_eq!(
        metadata.piece_hashes[0].sha1.as_deref(),
        Some("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d")
    );
    assert_eq!(metadata.piece_hashes[0].sha256, None);

    runtime
        .block_on(engine.remove_session(&session.handle, true))
        .unwrap();
    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn native_bridge_downloads_between_local_libtorrent_peers() {
    let sandbox = unique_sandbox();
    let seeder_dir = sandbox.join("seeder");
    let leecher_dir = sandbox.join("leecher");
    fs::create_dir_all(&seeder_dir).unwrap();
    fs::create_dir_all(&leecher_dir).unwrap();
    fs::write(sandbox.join("sample.torrent"), single_file_torrent()).unwrap();
    fs::write(seeder_dir.join("hello.txt"), b"hello").unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();

    runtime.block_on(async {
        let torrent_path = sandbox.join("sample.torrent");
        let seeder = local_engine();
        let leecher = local_engine();

        let seeder_session = seeder
            .start_session(paradown::p2p::TorrentEngineRequest {
                session_id: 1,
                spec: DownloadSpec::TorrentFile {
                    path: torrent_path.to_string_lossy().into_owned(),
                },
                download_dir: seeder_dir.clone(),
                requested_file_name: None,
                requested_file_path: None,
                rate_limit_kib_per_sec: None,
                resume: None,
                event_sender: None,
            })
            .await
            .unwrap();

        let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
        let leecher_session = leecher
            .start_session(paradown::p2p::TorrentEngineRequest {
                session_id: 2,
                spec: DownloadSpec::TorrentFile {
                    path: torrent_path.to_string_lossy().into_owned(),
                },
                download_dir: leecher_dir.clone(),
                requested_file_name: None,
                requested_file_path: None,
                rate_limit_kib_per_sec: None,
                resume: None,
                event_sender: Some(event_sender),
            })
            .await
            .unwrap();

        let seeder_port = wait_for_listen_port(&seeder).await;
        wait_for_finished_event(&leecher, &leecher_session.handle, seeder_port, &mut event_receiver)
            .await;

        assert_eq!(fs::read(leecher_dir.join("hello.txt")).unwrap(), b"hello");

        leecher
            .remove_session(&leecher_session.handle, true)
            .await
            .unwrap();
        seeder
            .remove_session(&seeder_session.handle, false)
            .await
            .unwrap();
    });

    let _ = fs::remove_dir_all(sandbox);
}

fn local_engine() -> LibtorrentRasterbarEngine {
    LibtorrentRasterbarEngine::new(LibtorrentEngineConfig {
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        listen_interfaces: Some("127.0.0.1:0".into()),
        ..LibtorrentEngineConfig::default()
    })
    .unwrap()
}

async fn wait_for_listen_port(engine: &LibtorrentRasterbarEngine) -> u16 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let port = engine.listen_port().unwrap();
        if port != 0 {
            return port;
        }
        assert!(
            Instant::now() < deadline,
            "libtorrent session did not open a listen port"
        );
        sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_finished_event(
    leecher: &LibtorrentRasterbarEngine,
    handle: &paradown::p2p::TorrentEngineHandle,
    seeder_port: u16,
    event_receiver: &mut mpsc::UnboundedReceiver<TorrentEngineEvent>,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let _ = leecher.connect_peer(handle, "127.0.0.1", seeder_port);

        tokio::select! {
            event = event_receiver.recv() => {
                match event {
                    Some(TorrentEngineEvent::Finished) => return,
                    Some(TorrentEngineEvent::Error(message)) => panic!("libtorrent error: {message}"),
                    Some(_) => {}
                    None => panic!("libtorrent event stream closed before finish"),
                }
            }
            _ = sleep(Duration::from_millis(250)) => {}
        }

        assert!(
            Instant::now() < deadline,
            "local libtorrent peer download did not finish"
        );
    }
}

fn unique_sandbox() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "paradown-libtorrent-native-{}-{nanos}",
        std::process::id()
    ))
}

fn single_file_torrent() -> Vec<u8> {
    let mut torrent = b"d4:infod6:lengthi5e4:name9:hello.txt12:piece lengthi16384e6:pieces20:"
        .to_vec();
    torrent.extend_from_slice(&[
        0xaa, 0xf4, 0xc6, 0x1d, 0xdc, 0xc5, 0xe8, 0xa2, 0xda, 0xbe, 0xde, 0x0f, 0x3b, 0x48,
        0x2c, 0xd9, 0xae, 0xa9, 0x43, 0x4d,
    ]);
    torrent.extend_from_slice(b"ee");
    torrent
}
