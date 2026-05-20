#![cfg(feature = "native-libtorrent")]

use paradown::download::{
    DownloadSpec, LibtorrentEngineConfig, TorrentEngine, TorrentEngineBackend, TorrentEngineEvent,
};
use paradown_libtorrent_engine::LibtorrentRasterbarEngine;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::sleep;

static SANDBOX_COUNTER: AtomicU64 = AtomicU64::new(0);

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
            swarm_hints: paradown::p2p::TorrentSwarmHints::default(),
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
                swarm_hints: paradown::p2p::TorrentSwarmHints::default(),
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
                swarm_hints: paradown::p2p::TorrentSwarmHints::default(),
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

#[test]
fn native_bridge_resolves_magnet_metadata_from_local_peer() {
    let sandbox = unique_sandbox();
    let seeder_dir = sandbox.join("seeder");
    let leecher_dir = sandbox.join("magnet-leecher");
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
                swarm_hints: paradown::p2p::TorrentSwarmHints::default(),
                resume: None,
                event_sender: None,
            })
            .await
            .unwrap();

        let magnet_uri = format!(
            "magnet:?xt=urn:btih:{}&dn=hello.txt",
            seeder_session.handle.external_id
        );
        let (event_sender, mut event_receiver) = mpsc::unbounded_channel();
        let leecher_session = leecher
            .start_session(paradown::p2p::TorrentEngineRequest {
                session_id: 2,
                spec: DownloadSpec::Magnet { uri: magnet_uri },
                download_dir: leecher_dir.clone(),
                requested_file_name: None,
                requested_file_path: None,
                rate_limit_kib_per_sec: None,
                swarm_hints: paradown::p2p::TorrentSwarmHints::default(),
                resume: None,
                event_sender: Some(event_sender),
            })
            .await
            .unwrap();

        assert!(leecher_session.metadata.is_none());

        let seeder_port = wait_for_listen_port(&seeder).await;
        let metadata = wait_for_metadata_and_finished(
            &leecher,
            &leecher_session.handle,
            seeder_port,
            &mut event_receiver,
        )
        .await;

        assert_eq!(metadata.name, "hello.txt");
        assert_eq!(metadata.total_size, 5);
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

#[test]
fn native_cli_smoke_prints_native_options() {
    let output = run_cli_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_paradown-libtorrent"))
            .arg("--help"),
        Duration::from_secs(5),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "CLI exited with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout,
        stderr
    );
    assert!(stdout.contains("paradown-libtorrent"));
    assert!(stdout.contains("--download-dir"));
    assert!(stdout.contains("--timeout-secs"));
    assert!(stdout.contains("--listen-interfaces"));
    assert!(stdout.contains("--peer"));
    assert!(stdout.contains("--tracker"));
    assert!(stdout.contains("--tracker-file"));
    assert!(stdout.contains("--web-seed"));
    assert!(stdout.contains("--discover-file"));
    assert!(stdout.contains("--discover-url"));
    assert!(stdout.contains("--disable-swarm-providers"));
    assert!(stdout.contains("--swarm-provider-cache-dir"));
    assert!(stdout.contains("--tracker-list-url"));
    assert!(stdout.contains("--index-url-template"));
    assert!(stdout.contains("--index-query"));
    assert!(stdout.contains("--index-kind"));
    assert!(stdout.contains("--swarm-max-trackers"));
}

#[test]
fn native_cli_timeout_exits_with_diagnostics_snapshot() {
    let sandbox = unique_sandbox();
    let download_dir = sandbox.join("downloads");
    fs::create_dir_all(&download_dir).unwrap();
    let torrent_path = sandbox.join("sample.torrent");
    fs::write(&torrent_path, single_file_torrent()).unwrap();

    let output = run_cli_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_paradown-libtorrent"))
            .arg("--download-dir")
            .arg(&download_dir)
            .arg("--storage-db")
            .arg(sandbox.join("downloads.db"))
            .arg("--timeout-secs")
            .arg("1")
            .arg(&torrent_path),
        Duration::from_secs(8),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(124),
        "stdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(stdout.contains("#1 "));
    assert!(stderr.contains("timed out after 1s"));
    assert!(stderr.contains("swarm"));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn native_cli_discovers_torrent_from_html_file() {
    let sandbox = unique_sandbox();
    let download_dir = sandbox.join("downloads");
    fs::create_dir_all(&download_dir).unwrap();
    let torrent_path = sandbox.join("sample.torrent");
    fs::write(&torrent_path, single_file_torrent()).unwrap();
    let discovery_path = sandbox.join("index.html");
    fs::write(
        &discovery_path,
        format!(
            r#"<html><a href="{}">sample</a></html>"#,
            torrent_path.display()
        ),
    )
    .unwrap();

    let output = run_cli_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_paradown-libtorrent"))
            .arg("--download-dir")
            .arg(&download_dir)
            .arg("--storage-db")
            .arg(sandbox.join("downloads.db"))
            .arg("--timeout-secs")
            .arg("1")
            .arg("--discover-file")
            .arg(&discovery_path),
        Duration::from_secs(8),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(124),
        "stdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(stdout.contains("#1 "));
    assert!(stdout.contains("sample.torrent") || stderr.contains("sample.torrent"));

    let _ = fs::remove_dir_all(sandbox);
}

#[test]
fn native_cli_prints_static_provider_candidates() {
    let sandbox = unique_sandbox();
    let download_dir = sandbox.join("downloads");
    fs::create_dir_all(&download_dir).unwrap();
    let torrent_path = sandbox.join("sample.torrent");
    fs::write(&torrent_path, single_file_torrent()).unwrap();
    let tracker_file = sandbox.join("trackers.txt");
    fs::write(&tracker_file, "udp://provider.example/announce\n").unwrap();

    let output = run_cli_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_paradown-libtorrent"))
            .arg("--download-dir")
            .arg(&download_dir)
            .arg("--storage-db")
            .arg(sandbox.join("downloads.db"))
            .arg("--timeout-secs")
            .arg("1")
            .arg("--tracker-file")
            .arg(&tracker_file)
            .arg(&torrent_path),
        Duration::from_secs(8),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(124),
        "stdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(stderr.contains("swarm provider 1 candidates from static"));
    assert!(stderr.contains("udp://provider.example/announce"));

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

fn run_cli_with_timeout(command: &mut Command, timeout: Duration) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            if let Some(mut pipe) = child.stdout.take() {
                let _ = pipe.read_to_end(&mut stdout);
            }
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_end(&mut stderr);
            }
            let _ = child.wait();
            panic!(
                "CLI timed out after {:?}\nstdout:\n{}\nstderr:\n{}",
                timeout,
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }

        std::thread::sleep(Duration::from_millis(50));
    }
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

async fn wait_for_metadata_and_finished(
    leecher: &LibtorrentRasterbarEngine,
    handle: &paradown::p2p::TorrentEngineHandle,
    seeder_port: u16,
    event_receiver: &mut mpsc::UnboundedReceiver<TorrentEngineEvent>,
) -> paradown::p2p::TorrentMetadata {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut metadata = None;
    loop {
        let _ = leecher.connect_peer(handle, "127.0.0.1", seeder_port);

        tokio::select! {
            event = event_receiver.recv() => {
                match event {
                    Some(TorrentEngineEvent::MetadataDiscovered(discovered)) => {
                        metadata = Some(discovered);
                    }
                    Some(TorrentEngineEvent::Finished) => {
                        return metadata.expect("magnet metadata before finished");
                    }
                    Some(TorrentEngineEvent::Error(message)) => panic!("libtorrent error: {message}"),
                    Some(_) => {}
                    None => panic!("libtorrent event stream closed before finish"),
                }
            }
            _ = sleep(Duration::from_millis(250)) => {}
        }

        assert!(
            Instant::now() < deadline,
            "local magnet metadata exchange did not finish"
        );
    }
}

fn unique_sandbox() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = SANDBOX_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "paradown-libtorrent-native-{}-{nanos}-{sequence}",
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
