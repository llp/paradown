mod commands;
mod render;

use self::commands::{Command, CommandTarget, help_lines, parse_command};
use self::render::{DashboardHandle, DashboardMessageTx, should_render_dashboard};
use clap::Parser;
use log::{LevelFilter, error, info, warn};
use paradown::download::{DownloadSpec, Event, Manager, TaskSnapshot};
use paradown::{Config, Error, init_logger_with_level};
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "paradown")]
#[command(about = "A multi-threaded download tool")]
pub(crate) struct Cli {
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

pub(crate) async fn run() -> Result<(), Error> {
    let cli = Cli::parse();
    let config = build_config(&cli)?;
    let dashboard_enabled = should_render_dashboard(cli.verbose);

    init_logger_with_level(select_log_level(&cli, dashboard_enabled));

    let manager = Manager::new(config.clone())?;
    manager.init().await?;

    let dashboard = if dashboard_enabled {
        Some(DashboardHandle::spawn(Arc::clone(&manager), cli.interactive))
    } else {
        None
    };

    let command_task = if cli.interactive {
        let message_tx = dashboard.as_ref().map(|handle| handle.message_tx());
        Some(spawn_command_loop(Arc::clone(&manager), message_tx))
    } else {
        None
    };
    let event_task = if dashboard_enabled {
        None
    } else {
        Some(spawn_event_reporter(Arc::clone(&manager)))
    };

    for url in prepare_urls(&cli.urls, config.shuffle) {
        let task_id = manager
            .add_download(DownloadSpec::parse(url.clone())?)
            .await?;
        manager.start_task(task_id).await?;
    }

    manager.wait_for_all_tasks().await?;

    if let Some(handle) = dashboard {
        handle.shutdown().await;
    }
    if let Some(task) = command_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(task) = event_task {
        task.abort();
        let _ = task.await;
    }

    Ok(())
}

fn select_log_level(cli: &Cli, dashboard_enabled: bool) -> LevelFilter {
    if dashboard_enabled {
        LevelFilter::Warn
    } else if cli.verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    }
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

fn prepare_urls(urls: &[String], shuffle: bool) -> Vec<String> {
    let mut urls = urls.to_vec();
    if shuffle {
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();
        urls.shuffle(&mut rng);
    }
    urls
}

fn spawn_event_reporter(manager: Arc<Manager>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = manager.subscribe_events();

        loop {
            match rx.recv().await {
                Ok(Event::Start(id)) => info!("Task #{id} started"),
                Ok(Event::Pause(id)) => info!("Task #{id} paused"),
                Ok(Event::Complete(id)) => info!("Task #{id} completed"),
                Ok(Event::Cancel(id)) => warn!("Task #{id} canceled"),
                Ok(Event::Delete(id)) => warn!("Task #{id} deleted"),
                Ok(Event::Error(id, err)) => error!("Task #{id} failed: {err}"),
                Ok(Event::Preparing(_)) | Ok(Event::Pending(_)) | Ok(Event::Progress { .. }) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!("Event reporter skipped {skipped} messages because it lagged behind");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

fn spawn_command_loop(
    manager: Arc<Manager>,
    message_tx: Option<DashboardMessageTx>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Some(message_tx) = &message_tx {
            for line in help_lines() {
                let _ = message_tx.send(line);
            }
        } else {
            for line in help_lines() {
                println!("{line}");
            }
        }

        use tokio::io::AsyncBufReadExt;

        let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let mut line = String::new();

        loop {
            match stdin.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let input = line.trim();
                    if !input.is_empty() {
                        let messages = match parse_command(input) {
                            Ok(command) => execute_command(&manager, command).await,
                            Err(err) => vec![err],
                        };
                        emit_messages(&message_tx, messages);
                    }
                    line.clear();
                }
                Err(err) => {
                    emit_messages(
                        &message_tx,
                        vec![format!("Failed to read interactive command: {err}")],
                    );
                    break;
                }
            }
        }
    })
}

async fn execute_command(manager: &Arc<Manager>, command: Command) -> Vec<String> {
    match command {
        Command::Help => help_lines(),
        Command::Status(target) => status_messages(manager, target).await,
        Command::Pause(target) => apply_task_command(manager, target, TaskControl::Pause).await,
        Command::Resume(target) => apply_task_command(manager, target, TaskControl::Resume).await,
        Command::Cancel(target) => apply_task_command(manager, target, TaskControl::Cancel).await,
        Command::SetRateLimit(limit_kbps) => {
            manager.set_rate_limit_kbps(limit_kbps).await;
            vec![format!(
                "Global rate limit set to {}",
                manager
                    .current_rate_limit_kbps()
                    .map(|value| format!("{value} KB/s"))
                    .unwrap_or_else(|| "unlimited".to_string())
            )]
        }
    }
}

fn emit_messages(message_tx: &Option<DashboardMessageTx>, messages: Vec<String>) {
    if let Some(message_tx) = message_tx {
        for message in messages {
            let _ = message_tx.send(message);
        }
    } else {
        for message in messages {
            println!("{message}");
        }
    }
}

async fn status_messages(manager: &Arc<Manager>, target: CommandTarget) -> Vec<String> {
    let mut tasks = resolve_target_tasks(manager, target);
    tasks.sort_by_key(|task| task.id);

    if tasks.is_empty() {
        return vec!["No matching tasks".into()];
    }

    let mut messages = vec![format!(
        "Rate limit: {}",
        manager
            .current_rate_limit_kbps()
            .map(|value| format!("{value} KB/s"))
            .unwrap_or_else(|| "unlimited".to_string())
    )];
    for task in tasks {
        let snapshot = task.snapshot().await;
        messages.push(format_status_line(&snapshot));
    }
    messages
}

async fn apply_task_command(
    manager: &Arc<Manager>,
    target: CommandTarget,
    control: TaskControl,
) -> Vec<String> {
    let task_ids = match target {
        CommandTarget::All => return apply_global_command(manager, control).await,
        CommandTarget::Tasks(ids) => ids,
    };

    let mut messages = Vec::new();
    for task_id in task_ids {
        let result = match control {
            TaskControl::Pause => manager.pause_task(task_id).await.map(|_| ()),
            TaskControl::Resume => manager.resume_task(task_id).await.map(|_| ()),
            TaskControl::Cancel => manager.cancel_task(task_id).await.map(|_| ()),
        };

        match result {
            Ok(()) => messages.push(format!("{} task #{task_id}", control.past_tense_label())),
            Err(err) => messages.push(format!(
                "Failed to {} task #{task_id}: {err}",
                control.verb_label()
            )),
        }
    }
    messages
}

async fn apply_global_command(manager: &Arc<Manager>, control: TaskControl) -> Vec<String> {
    let result = match control {
        TaskControl::Pause => manager.pause_all().await,
        TaskControl::Resume => manager.resume_all().await,
        TaskControl::Cancel => manager.cancel_all().await,
    };

    match result {
        Ok(()) => vec![format!("{} all tasks", control.past_tense_label())],
        Err(err) => vec![format!("Failed to {} all tasks: {err}", control.verb_label())],
    }
}

fn resolve_target_tasks(manager: &Arc<Manager>, target: CommandTarget) -> Vec<Arc<paradown::Task>> {
    match target {
        CommandTarget::All => manager.get_all_tasks(),
        CommandTarget::Tasks(ids) => ids
            .into_iter()
            .filter_map(|task_id| manager.get_task_by_id(task_id))
            .collect(),
    }
}

fn format_status_line(snapshot: &TaskSnapshot) -> String {
    let progress = if snapshot.total_size > 0 {
        format!(
            "{:.1}%",
            snapshot.downloaded_size as f64 / snapshot.total_size as f64 * 100.0
        )
    } else {
        "-".into()
    };
    let piece_label = if snapshot.piece_count > 0 {
        format!(" pieces:{}/{}", snapshot.completed_pieces, snapshot.piece_count)
    } else {
        String::new()
    };

    format!(
        "#{} {} {} {}/{}{}",
        snapshot.id,
        snapshot.status,
        progress,
        snapshot.downloaded_size,
        snapshot.total_size,
        piece_label
    )
}

#[derive(Clone, Copy)]
enum TaskControl {
    Pause,
    Resume,
    Cancel,
}

impl TaskControl {
    fn verb_label(self) -> &'static str {
        match self {
            TaskControl::Pause => "pause",
            TaskControl::Resume => "resume",
            TaskControl::Cancel => "cancel",
        }
    }

    fn past_tense_label(self) -> &'static str {
        match self {
            TaskControl::Pause => "Paused",
            TaskControl::Resume => "Resumed",
            TaskControl::Cancel => "Canceled",
        }
    }
}
