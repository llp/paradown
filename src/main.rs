use clap::Parser;
use log::{error, info, warn};
use paradown::cli::{Command, InteractiveMode, print_help};
use paradown::download::{DownloadSpec, Event, Manager};
use paradown::{Config, Error, init_logger};
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "paradown")]
#[command(about = "A multi-threaded download tool")]
struct Cli {
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(short, long, value_name = "N")]
    workers: Option<usize>,

    #[arg(long, value_name = "N")]
    max_concurrent: Option<usize>,

    #[arg(short, long, value_name = "DIR")]
    download_dir: Option<PathBuf>,

    #[arg(long, value_name = "KBPS")]
    rate_limit_kbps: Option<u64>,

    #[arg(short, long)]
    shuffle: bool,

    #[arg(short, long)]
    verbose: bool,

    #[arg(long)]
    interactive: bool,

    #[arg(short = 'u', long = "urls", value_name = "URL", num_args = 1.., required = true)]
    urls: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();
    let config = build_config(&cli)?;

    init_logger(cli.verbose || config.debug);

    info!(
        "Starting download process with {} workers (max_concurrent = {})",
        config.segments_per_task, config.concurrent_tasks
    );
    info!("Download directory: {}", config.download_dir.display());
    info!(
        "Rate limit: {}",
        config
            .rate_limit_kbps
            .map(|value| format!("{} KB/s", value))
            .unwrap_or_else(|| "unlimited".to_string())
    );
    info!("Interactive mode: {}", cli.interactive);

    let manager = Manager::new(config)?;
    manager.init().await?;

    let event_task = spawn_event_reporter(Arc::clone(&manager));
    let (interactive_task, command_task) = if cli.interactive {
        let (mut interactive, command_rx) = InteractiveMode::new();
        let command_task = spawn_command_handler(Arc::clone(&manager), command_rx);
        let interactive_task = tokio::spawn(async move {
            interactive.run().await;
        });
        (Some(interactive_task), Some(command_task))
    } else {
        (None, None)
    };

    for url in &cli.urls {
        let task_id = manager
            .add_download(DownloadSpec::parse(url.clone())?)
            .await?;
        manager.start_task(task_id).await?;
    }

    manager.wait_for_all_tasks().await?;

    if let Some(task) = interactive_task {
        task.abort();
    }
    if let Some(task) = command_task {
        task.abort();
    }
    event_task.abort();

    info!("Download process completed successfully");
    Ok(())
}

fn build_config(cli: &Cli) -> Result<Config, Error> {
    let mut config = match &cli.config {
        Some(path) => Config::from_file(path)?,
        None => Config::default(),
    };

    if let Some(download_dir) = &cli.download_dir {
        config.download_dir = download_dir.clone();
    }
    if let Some(workers) = cli.workers {
        config.segments_per_task = workers;
        if cli.max_concurrent.is_none() {
            config.concurrent_tasks = workers;
        }
    }
    if let Some(max_concurrent) = cli.max_concurrent {
        config.concurrent_tasks = max_concurrent;
    }
    if cli.shuffle {
        config.shuffle = true;
    }
    if let Some(rate_limit_kbps) = cli.rate_limit_kbps {
        config.rate_limit_kbps = NonZeroU64::new(rate_limit_kbps);
    }

    config.validate().map_err(Error::from)?;
    Ok(config)
}

fn spawn_event_reporter(manager: Arc<Manager>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = manager.subscribe_events();

        loop {
            match rx.recv().await {
                Ok(Event::Start(id)) => info!("任务 {id} 开始"),
                Ok(Event::Pause(id)) => info!("任务 {id} 已暂停"),
                Ok(Event::Complete(id)) => info!("任务 {id} 已完成"),
                Ok(Event::Cancel(id)) => warn!("任务 {id} 已取消"),
                Ok(Event::Delete(id)) => warn!("任务 {id} 已删除"),
                Ok(Event::Error(id, err)) => error!("任务 {id} 出错: {err}"),
                Ok(Event::Preparing(_)) | Ok(Event::Pending(_)) | Ok(Event::Progress { .. }) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("下载事件消费落后，跳过了 {skipped} 条事件");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

fn spawn_command_handler(
    manager: Arc<Manager>,
    mut command_rx: tokio::sync::mpsc::Receiver<Command>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(command) = command_rx.recv().await {
            if let Err(err) = handle_command(&manager, command).await {
                error!("Interactive command failed: {err}");
            }
        }
    })
}

async fn handle_command(manager: &Arc<Manager>, command: Command) -> Result<(), Error> {
    match command {
        Command::Help => {
            print_help();
            Ok(())
        }
        Command::Status => {
            print_task_summary(manager).await;
            Ok(())
        }
        Command::PauseAll => manager.pause_all().await.map(|_| ()),
        Command::ResumeAll => manager.resume_all().await.map(|_| ()),
        Command::CancelAll => manager.cancel_all().await.map(|_| ()),
        Command::SetRateLimit(limit_kbps) => {
            manager.set_rate_limit_kbps(limit_kbps).await;
            let label = manager
                .current_rate_limit_kbps()
                .map(|value| format!("{value} KB/s"))
                .unwrap_or_else(|| "unlimited".to_string());
            info!("Global rate limit updated to {label}");
            Ok(())
        }
    }
}

async fn print_task_summary(manager: &Arc<Manager>) {
    let mut tasks = manager.get_all_tasks();
    tasks.sort_by_key(|task| task.id);

    println!(
        "Rate limit: {}",
        manager
            .current_rate_limit_kbps()
            .map(|value| format!("{value} KB/s"))
            .unwrap_or_else(|| "unlimited".to_string())
    );

    if tasks.is_empty() {
        println!("No tasks");
        return;
    }

    for task in tasks {
        let snapshot = task.snapshot().await;
        println!(
            "#{} {} {} {}/{}",
            snapshot.id,
            snapshot.status,
            snapshot.url,
            snapshot.downloaded_size,
            snapshot.total_size
        );
    }
}
