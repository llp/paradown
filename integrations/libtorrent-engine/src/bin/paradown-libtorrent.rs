use clap::Parser;
use paradown::download::{DownloadSpec, Event, Manager, TorrentEngineHandle};
use paradown::{Backend, Config, init_logger_with_level};
use paradown_libtorrent_engine::LibtorrentRasterbarEngine;
use std::collections::HashSet;
use std::error::Error as StdError;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

type Result<T> = std::result::Result<T, Box<dyn StdError + Send + Sync>>;

#[derive(Parser, Debug)]
#[command(name = "paradown-libtorrent")]
#[command(about = "Native libtorrent-backed paradown runner for .torrent and magnet tasks")]
struct Cli {
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(short, long, value_name = "DIR")]
    download_dir: Option<PathBuf>,

    #[arg(long, value_name = "DB")]
    storage_db: Option<PathBuf>,

    #[arg(long, value_name = "KIB_PER_SEC")]
    rate_limit_kib: Option<u64>,

    #[arg(long, value_name = "INTERFACES")]
    listen_interfaces: Option<String>,

    #[arg(long = "peer", value_name = "HOST:PORT")]
    peers: Vec<String>,

    #[arg(long)]
    disable_dht: bool,

    #[arg(long)]
    disable_lsd: bool,

    #[arg(long)]
    disable_upnp: bool,

    #[arg(long)]
    disable_natpmp: bool,

    #[arg(short = 'u', long = "urls", value_name = "TORRENT_OR_MAGNET", num_args = 1..)]
    urls: Vec<String>,

    #[arg(value_name = "TORRENT_OR_MAGNET")]
    locators: Vec<String>,
}

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("paradown-libtorrent: failed to create tokio runtime: {err}");
            return ExitCode::from(1);
        }
    };

    match runtime.block_on(run()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("paradown-libtorrent: {err}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let config = build_config(&cli)?;
    init_logger_with_level(config.log_level.as_level_filter());

    let locators = collect_locators(&cli);
    if locators.is_empty() {
        return Err("provide at least one .torrent path or magnet URI".into());
    }
    let peers = parse_peers(&cli.peers)?;

    let engine = Arc::new(LibtorrentRasterbarEngine::new(
        config.p2p.libtorrent.clone(),
    )?);
    let manager = Manager::new_with_torrent_engine(config.clone(), engine.clone())?;
    manager.init().await?;

    let event_task = spawn_event_reporter(Arc::clone(&manager));
    let mut task_ids = Vec::with_capacity(locators.len());
    let mut torrent_handles = Vec::with_capacity(locators.len());
    for locator in locators {
        let task_id = manager.add_download(DownloadSpec::parse(locator)?).await?;
        manager.start_task(task_id).await?;
        if let Some(session) = manager.get_session(task_id)
            && let Some(handle) = session.torrent_handle().await
        {
            torrent_handles.push(handle);
        }
        task_ids.push(task_id);
    }
    if !peers.is_empty() && torrent_handles.is_empty() {
        return Err("explicit peers were provided, but no torrent handles are available".into());
    }
    let peer_task = spawn_peer_connector(Arc::clone(&engine), torrent_handles, peers);

    manager.wait_for_all_tasks().await?;
    if let Some(peer_task) = peer_task {
        peer_task.abort();
        let _ = peer_task.await;
    }
    event_task.abort();
    let _ = event_task.await;

    let mut exit_code = ExitCode::SUCCESS;
    for task_id in task_ids {
        let Some(session) = manager.get_session(task_id) else {
            exit_code = ExitCode::from(1);
            continue;
        };
        let snapshot = session.snapshot().await;
        println!(
            "#{id} {status} {downloaded}/{total} {path}",
            id = snapshot.id,
            status = snapshot.status,
            downloaded = snapshot.downloaded_size,
            total = if snapshot.total_size_known {
                snapshot.total_size.to_string()
            } else {
                "?".into()
            },
            path = snapshot
                .file_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| snapshot.locator.clone())
        );
        if snapshot.status != "Completed" {
            exit_code = ExitCode::from(1);
        }
    }

    Ok(exit_code)
}

fn build_config(cli: &Cli) -> Result<Config> {
    let mut config = match &cli.config {
        Some(path) => Config::from_file(path)?,
        None => Config::default(),
    };
    config.apply_env_overrides()?;

    config.p2p.enabled = true;
    if let Some(download_dir) = &cli.download_dir {
        config.download_dir = download_dir.clone();
    }
    if let Some(storage_db) = &cli.storage_db {
        config.storage_backend = Backend::Sqlite(storage_db.clone());
    }
    if let Some(rate_limit_kib) = cli.rate_limit_kib {
        config.rate_limit_kib_per_sec = NonZeroU64::new(rate_limit_kib);
    }
    if let Some(listen_interfaces) = &cli.listen_interfaces {
        config.p2p.libtorrent.listen_interfaces = Some(listen_interfaces.clone());
    }
    if cli.disable_dht {
        config.p2p.libtorrent.enable_dht = false;
    }
    if cli.disable_lsd {
        config.p2p.libtorrent.enable_lsd = false;
    }
    if cli.disable_upnp {
        config.p2p.libtorrent.enable_upnp = false;
    }
    if cli.disable_natpmp {
        config.p2p.libtorrent.enable_natpmp = false;
    }

    config.validate()?;
    Ok(config)
}

fn collect_locators(cli: &Cli) -> Vec<String> {
    cli.urls
        .iter()
        .chain(cli.locators.iter())
        .cloned()
        .collect()
}

#[derive(Clone, Debug)]
struct PeerEndpoint {
    host: String,
    port: u16,
}

fn parse_peers(values: &[String]) -> Result<Vec<PeerEndpoint>> {
    values.iter().map(|value| parse_peer(value)).collect()
}

fn parse_peer(value: &str) -> Result<PeerEndpoint> {
    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| format!("peer must be HOST:PORT, got '{value}'"))?;
    if host.trim().is_empty() {
        return Err(format!("peer host cannot be blank in '{value}'").into());
    }
    let port = port
        .parse::<u16>()
        .map_err(|err| format!("invalid peer port in '{value}': {err}"))?;
    Ok(PeerEndpoint {
        host: host.to_string(),
        port,
    })
}

fn spawn_peer_connector(
    engine: Arc<LibtorrentRasterbarEngine>,
    handles: Vec<TorrentEngineHandle>,
    peers: Vec<PeerEndpoint>,
) -> Option<tokio::task::JoinHandle<()>> {
    if handles.is_empty() || peers.is_empty() {
        return None;
    }

    Some(tokio::spawn(async move {
        let mut reported_errors = HashSet::new();
        loop {
            for handle in &handles {
                for peer in &peers {
                    if let Err(err) = engine.connect_peer(handle, &peer.host, peer.port) {
                        let key = format!("{}:{}:{}", handle.external_id, peer.host, peer.port);
                        if reported_errors.insert(key) {
                            eprintln!(
                                "#{} peer {}:{} failed: {}",
                                handle.external_id, peer.host, peer.port, err
                            );
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }))
}

fn spawn_event_reporter(manager: Arc<Manager>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = manager.subscribe_events();
        while let Ok(event) = rx.recv().await {
            match event {
                Event::Pending(id) => eprintln!("#{id} pending"),
                Event::Preparing(id) => eprintln!("#{id} preparing"),
                Event::Start(id) => eprintln!("#{id} started"),
                Event::Pause(id) => eprintln!("#{id} paused"),
                Event::Progress {
                    id,
                    downloaded,
                    total,
                } => eprintln!("#{id} progress {downloaded}/{total}"),
                Event::Complete(id) => eprintln!("#{id} completed"),
                Event::Error(id, err) => eprintln!("#{id} failed: {err}"),
                Event::Cancel(id) => eprintln!("#{id} canceled"),
                Event::Delete(id) => eprintln!("#{id} deleted"),
            }
        }
    })
}
