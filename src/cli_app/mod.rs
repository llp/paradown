mod commands;
mod render;

use self::commands::{Command, CommandTarget, help_lines, parse_command};
use self::render::{DashboardHandle, DashboardMessageTx, PlainTextHandle};
use clap::Parser;
use log::{LevelFilter, error, info, warn};
use paradown::download::{DownloadSpec, Event, Manager, TaskSnapshot};
use paradown::{Config, Error, init_logger_with_level};
use std::io::IsTerminal;
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
    let output_mode = select_output_mode(&cli);

    init_logger_with_level(select_log_level(output_mode));

    let manager = Manager::new(config.clone())?;
    manager.init().await?;

    let dashboard = match output_mode {
        OutputMode::Dashboard => Some(DashboardHandle::spawn(
            Arc::clone(&manager),
            cli.interactive,
        )),
        _ => None,
    };
    let plain_reporter = match output_mode {
        OutputMode::PlainText => Some(PlainTextHandle::spawn(Arc::clone(&manager))),
        _ => None,
    };

    let command_task = if cli.interactive {
        let message_tx = dashboard.as_ref().map(|handle| handle.message_tx());
        Some(spawn_command_loop(Arc::clone(&manager), message_tx))
    } else {
        None
    };
    let event_task = match output_mode {
        OutputMode::LogOnly => Some(spawn_event_reporter(Arc::clone(&manager))),
        _ => None,
    };

    for url in prepare_urls(&cli.urls, config.shuffle) {
        let task_id = manager
            .add_download(DownloadSpec::parse(url.clone())?)
            .await?;
        manager.start_task(task_id).await?;
    }

    manager.wait_for_all_tasks().await?;
    let completion_summary = build_completion_summary(&manager).await;

    if let Some(handle) = dashboard {
        handle.shutdown().await;
    }
    if let Some(handle) = plain_reporter {
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

    print_completion_summary(&completion_summary);

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputMode {
    Dashboard,
    PlainText,
    LogOnly,
}

fn select_output_mode(cli: &Cli) -> OutputMode {
    if cli.verbose {
        OutputMode::LogOnly
    } else if std::io::stdout().is_terminal() {
        OutputMode::Dashboard
    } else {
        OutputMode::PlainText
    }
}

fn select_log_level(output_mode: OutputMode) -> LevelFilter {
    if matches!(output_mode, OutputMode::LogOnly) {
        LevelFilter::Debug
    } else {
        LevelFilter::Warn
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
        Command::Retry(target) => apply_task_command(manager, target, TaskControl::Retry).await,
        Command::Cancel(target) => apply_task_command(manager, target, TaskControl::Cancel).await,
        Command::Delete(target) => apply_task_command(manager, target, TaskControl::Delete).await,
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
        CommandTarget::All if !matches!(control, TaskControl::Retry) => {
            return apply_global_command(manager, control).await;
        }
        CommandTarget::All => manager
            .get_all_tasks()
            .into_iter()
            .map(|task| task.id)
            .collect(),
        CommandTarget::Tasks(ids) => ids,
    };

    let mut messages = Vec::new();
    for task_id in task_ids {
        let result = match control {
            TaskControl::Pause => manager.pause_task(task_id).await.map(|_| ()),
            TaskControl::Resume => manager.resume_task(task_id).await.map(|_| ()),
            TaskControl::Retry => manager.start_task(task_id).await.map(|_| ()),
            TaskControl::Cancel => manager.cancel_task(task_id).await.map(|_| ()),
            TaskControl::Delete => manager.delete_task(task_id).await.map(|_| ()),
        };

        match result {
            Ok(()) => messages.push(format!("{} task #{task_id}", control.past_tense_label())),
            Err(err) => messages.push(describe_task_error(manager, task_id, control, &err).await),
        }
    }
    messages
}

async fn apply_global_command(manager: &Arc<Manager>, control: TaskControl) -> Vec<String> {
    let result = match control {
        TaskControl::Pause => manager.pause_all().await,
        TaskControl::Resume => manager.resume_all().await,
        TaskControl::Retry => Ok(()),
        TaskControl::Cancel => manager.cancel_all().await,
        TaskControl::Delete => manager.delete_all().await,
    };

    match result {
        Ok(()) => vec![format!("{} all tasks", control.past_tense_label())],
        Err(err) => vec![format!(
            "Failed to {} all tasks: {err}",
            control.verb_label()
        )],
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
        format!(
            " pieces:{}/{}",
            snapshot.completed_pieces, snapshot.piece_count
        )
    } else {
        String::new()
    };

    format!(
        "#{} {} {} {}/{}{} {}",
        snapshot.id,
        snapshot.status,
        progress,
        format_bytes(snapshot.downloaded_size),
        format_bytes(snapshot.total_size),
        piece_label,
        snapshot
            .file_name
            .as_deref()
            .unwrap_or(snapshot.url.as_str())
    )
}

#[derive(Clone, Copy)]
enum TaskControl {
    Pause,
    Resume,
    Retry,
    Cancel,
    Delete,
}

impl TaskControl {
    fn verb_label(self) -> &'static str {
        match self {
            TaskControl::Pause => "pause",
            TaskControl::Resume => "resume",
            TaskControl::Retry => "retry",
            TaskControl::Cancel => "cancel",
            TaskControl::Delete => "delete",
        }
    }

    fn past_tense_label(self) -> &'static str {
        match self {
            TaskControl::Pause => "Paused",
            TaskControl::Resume => "Resumed",
            TaskControl::Retry => "Retried",
            TaskControl::Cancel => "Canceled",
            TaskControl::Delete => "Deleted",
        }
    }
}

async fn describe_task_error(
    manager: &Arc<Manager>,
    task_id: u32,
    control: TaskControl,
    err: &Error,
) -> String {
    let available_ids = {
        let mut ids: Vec<_> = manager
            .get_all_tasks()
            .into_iter()
            .map(|task| task.id)
            .collect();
        ids.sort_unstable();
        ids
    };

    if matches!(err, Error::TaskNotFound(_)) {
        return if available_ids.is_empty() {
            format!(
                "Failed to {} task #{task_id}: task does not exist and there are no active tasks",
                control.verb_label()
            )
        } else {
            format!(
                "Failed to {} task #{task_id}: task does not exist. Available task ids: {}",
                control.verb_label(),
                available_ids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
    }

    let Some(task) = manager.get_task_by_id(task_id) else {
        return format!("Failed to {} task #{task_id}: {err}", control.verb_label());
    };
    let snapshot = task.snapshot().await;
    let hint = status_hint(control, &snapshot);
    if hint.is_empty() {
        format!(
            "Failed to {} task #{} (current state: {}): {}",
            control.verb_label(),
            task_id,
            snapshot.status,
            err
        )
    } else {
        format!(
            "Failed to {} task #{} (current state: {}): {}. Hint: {}",
            control.verb_label(),
            task_id,
            snapshot.status,
            err,
            hint
        )
    }
}

fn status_hint(control: TaskControl, snapshot: &TaskSnapshot) -> &'static str {
    match (control, snapshot.status.as_str()) {
        (TaskControl::Retry, "Completed") => {
            "completed tasks are not retried automatically; use delete <id> and add the URL again if you want a fresh download"
        }
        (TaskControl::Pause, "Paused") => "the task is already paused",
        (TaskControl::Resume, "Running") => "the task is already running",
        (TaskControl::Resume, "Completed") => "completed tasks cannot be resumed",
        (TaskControl::Cancel, "Canceled") => "the task is already canceled",
        (TaskControl::Delete, "Deleted") => "the task has already been deleted",
        _ => "",
    }
}

struct CompletionSummary {
    snapshots: Vec<TaskSnapshot>,
    completed: usize,
    failed: usize,
    canceled: usize,
    deleted: usize,
    total_downloaded: u64,
    total_size: u64,
}

async fn build_completion_summary(manager: &Arc<Manager>) -> CompletionSummary {
    let mut tasks = manager.get_all_tasks();
    tasks.sort_by_key(|task| task.id);

    let mut snapshots = Vec::with_capacity(tasks.len());
    let mut completed = 0usize;
    let mut failed = 0usize;
    let mut canceled = 0usize;
    let mut deleted = 0usize;
    let mut total_downloaded = 0u64;
    let mut total_size = 0u64;

    for task in tasks {
        let snapshot = task.snapshot().await;
        total_downloaded = total_downloaded.saturating_add(snapshot.downloaded_size);
        total_size = total_size.saturating_add(snapshot.total_size);
        match snapshot.status.as_str() {
            "Completed" => completed += 1,
            "Canceled" => canceled += 1,
            "Deleted" => deleted += 1,
            status if status.starts_with("Failed") => failed += 1,
            _ => {}
        }
        snapshots.push(snapshot);
    }

    CompletionSummary {
        snapshots,
        completed,
        failed,
        canceled,
        deleted,
        total_downloaded,
        total_size,
    }
}

fn print_completion_summary(summary: &CompletionSummary) {
    println!(
        "Final summary: tasks={} completed={} failed={} canceled={} deleted={} total={} / {}",
        summary.snapshots.len(),
        summary.completed,
        summary.failed,
        summary.canceled,
        summary.deleted,
        format_bytes(summary.total_downloaded),
        format_bytes(summary.total_size),
    );
    for snapshot in &summary.snapshots {
        println!("  {}", format_status_line(snapshot));
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;

    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
