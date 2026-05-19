use clap::Parser;
use paradown::download::{DownloadSpec, Event, Manager};
use paradown::{Backend, Config, init_logger_with_level};
use paradown_libtorrent_engine::LibtorrentRasterbarEngine;
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

    let engine = Arc::new(LibtorrentRasterbarEngine::new(
        config.p2p.libtorrent.clone(),
    )?);
    let manager = Manager::new_with_torrent_engine(config.clone(), engine)?;
    manager.init().await?;

    let event_task = spawn_event_reporter(Arc::clone(&manager));
    let mut task_ids = Vec::with_capacity(locators.len());
    for locator in locators {
        let task_id = manager.add_download(DownloadSpec::parse(locator)?).await?;
        manager.start_task(task_id).await?;
        task_ids.push(task_id);
    }

    manager.wait_for_all_tasks().await?;
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
