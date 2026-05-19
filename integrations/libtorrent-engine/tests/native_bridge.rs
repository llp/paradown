#![cfg(feature = "native-libtorrent")]

use paradown::download::{DownloadSpec, LibtorrentEngineConfig, TorrentEngine, TorrentEngineBackend};
use paradown_libtorrent_engine::LibtorrentRasterbarEngine;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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

    runtime
        .block_on(engine.remove_session(&session.handle, true))
        .unwrap();
    let _ = fs::remove_dir_all(sandbox);
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
