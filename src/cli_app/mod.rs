mod commands;
mod render;

use self::commands::{Command, CommandTarget, StatusFilter, help_lines, parse_command};
use self::render::{DashboardHandle, DashboardMessageTx, JsonHandle, PlainTextHandle};
use clap::Parser;
use log::{LevelFilter, error, info, warn};
use paradown::download::{DownloadSpec, Event, Manager, Session, SessionSnapshot};
use paradown::{Config, Error, HttpAuth, HttpHeader, init_logger_with_level};
use serde::Serialize;
use std::io::IsTerminal;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::process::ExitCode;
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

    #[arg(
        long = "rate-limit-kib",
        alias = "rate-limit-kbps",
        value_name = "KIB_PER_SEC"
    )]
    rate_limit_kib_per_sec: Option<u64>,

    #[arg(long = "header", value_name = "NAME:VALUE")]
    headers: Vec<String>,

    #[arg(long, value_name = "COOKIE")]
    cookie: Option<String>,

    #[arg(long = "user-agent", value_name = "USER_AGENT")]
    user_agent: Option<String>,

    #[arg(long = "basic-auth", value_name = "USER[:PASS]")]
    basic_auth: Option<String>,

    #[arg(long = "bearer-token", value_name = "TOKEN")]
    bearer_token: Option<String>,

    #[arg(long = "http-proxy", value_name = "URL")]
    http_proxy: Option<String>,

    #[arg(long = "https-proxy", value_name = "URL")]
    https_proxy: Option<String>,

    #[arg(long = "no-proxy", value_name = "PATTERN")]
    no_proxy: Option<String>,

    #[arg(long = "no-env-proxy")]
    no_env_proxy: bool,

    #[arg(long = "cookie-store")]
    cookie_store: bool,

    #[arg(long = "cookie-jar", value_name = "FILE")]
    cookie_jar: Option<PathBuf>,

    #[arg(long = "insecure-tls")]
    insecure_tls: bool,

    #[arg(long = "ca-cert", value_name = "PEM_FILE")]
    ca_cert: Option<PathBuf>,

    #[arg(long = "client-identity", value_name = "PEM_FILE")]
    client_identity: Option<PathBuf>,

    #[arg(short, long = "shuffle-tasks", alias = "shuffle")]
    shuffle_tasks: bool,

    #[arg(short, long)]
    verbose: bool,

    #[arg(long)]
    json: bool,

    #[arg(long)]
    interactive: bool,

    #[arg(long = "urls-file", value_name = "FILE")]
    urls_file: Vec<PathBuf>,

    #[arg(
        long = "completion-hook",
        alias = "on-complete",
        value_name = "COMMAND"
    )]
    completion_hook: Option<String>,

    #[arg(short = 'u', long = "urls", value_name = "URL", num_args = 1..)]
    urls: Vec<String>,
}

pub(crate) async fn run() -> Result<ExitCode, Error> {
    let cli = Cli::parse();
    let config = build_config(&cli)?;
    let output_mode = select_output_mode(&cli);

    init_logger_with_level(select_log_level(&config, output_mode));

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
    let json_reporter = match output_mode {
        OutputMode::Json => Some(JsonHandle::spawn(Arc::clone(&manager))),
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

    let urls = collect_urls(&cli).await?;
    if urls.is_empty() {
        return Err(Error::ConfigError(
            "at least one URL must be provided via --urls or --urls-file".into(),
        ));
    }

    for url in prepare_urls(&urls, config.shuffle_tasks) {
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
    if let Some(handle) = json_reporter {
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

    match output_mode {
        OutputMode::Json => print_json_completion_summary(&completion_summary),
        _ => print_completion_summary(&completion_summary),
    }
    run_completion_hook(&config, &completion_summary).await;

    Ok(exit_code_from_summary(&completion_summary))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputMode {
    Dashboard,
    PlainText,
    Json,
    LogOnly,
}

fn select_output_mode(cli: &Cli) -> OutputMode {
    if cli.json {
        OutputMode::Json
    } else if cli.verbose {
        OutputMode::LogOnly
    } else if std::io::stdout().is_terminal() {
        OutputMode::Dashboard
    } else {
        OutputMode::PlainText
    }
}

fn select_log_level(config: &Config, output_mode: OutputMode) -> LevelFilter {
    match output_mode {
        OutputMode::LogOnly => config.log_level.as_level_filter(),
        OutputMode::Dashboard | OutputMode::PlainText | OutputMode::Json => {
            if matches!(config.log_level, paradown::LogLevel::Error) {
                LevelFilter::Error
            } else {
                LevelFilter::Warn
            }
        }
    }
}

fn build_config(cli: &Cli) -> Result<Config, Error> {
    let mut config = match &cli.config {
        Some(path) => Config::from_file(path)?,
        None => Config::default(),
    };
    config.apply_env_overrides()?;

    if let Some(download_dir) = &cli.download_dir {
        config.download_dir = download_dir.clone();
    }
    if cli.json && cli.interactive {
        return Err(Error::ConfigError(
            "--json and --interactive cannot be used together".into(),
        ));
    }
    if cli.json && cli.verbose {
        return Err(Error::ConfigError(
            "--json and --verbose cannot be used together".into(),
        ));
    }
    if let Some(workers) = cli.workers {
        config.segments_per_task = workers;
    }
    if let Some(max_concurrent) = cli.max_concurrent {
        config.concurrent_tasks = max_concurrent;
    }
    if cli.shuffle_tasks {
        config.shuffle_tasks = true;
    }
    if let Some(rate_limit_kib_per_sec) = cli.rate_limit_kib_per_sec {
        config.rate_limit_kib_per_sec = NonZeroU64::new(rate_limit_kib_per_sec);
    }
    if let Some(command) = &cli.completion_hook {
        config.completion_hook = Some(command.clone());
    }
    if cli.no_env_proxy {
        config.http.client.proxy.use_env_proxy = false;
    }
    if cli.cookie_store {
        config.http.client.cookie_store = true;
    }
    if let Some(path) = &cli.cookie_jar {
        config.http.client.cookie_store = true;
        config.http.client.cookie_jar_path = Some(path.clone());
    }
    if let Some(proxy) = &cli.http_proxy {
        config.http.client.proxy.http_proxy = Some(proxy.clone());
    }
    if let Some(proxy) = &cli.https_proxy {
        config.http.client.proxy.https_proxy = Some(proxy.clone());
    }
    if let Some(no_proxy) = &cli.no_proxy {
        config.http.client.proxy.no_proxy = Some(no_proxy.clone());
    }
    if cli.insecure_tls {
        config.http.client.tls.insecure_skip_verify = true;
    }
    if let Some(path) = &cli.ca_cert {
        config.http.client.tls.ca_certificate_pem = Some(path.clone());
    }
    if let Some(path) = &cli.client_identity {
        config.http.client.tls.client_identity_pem = Some(path.clone());
    }
    if !cli.headers.is_empty() {
        config.http.request.headers = parse_cli_headers(&cli.headers)?;
    }
    if let Some(cookie) = &cli.cookie {
        config.http.request.cookie = Some(cookie.clone());
    }
    if let Some(user_agent) = &cli.user_agent {
        config.http.request.user_agent = Some(user_agent.clone());
    }
    if let Some(auth) = &cli.basic_auth {
        config.http.request.auth = Some(parse_basic_auth(auth)?);
    }
    if let Some(token) = &cli.bearer_token {
        config.http.request.auth = Some(HttpAuth::Bearer {
            token: token.clone(),
        });
    }

    config.validate().map_err(Error::from)?;
    Ok(config)
}

fn parse_cli_headers(values: &[String]) -> Result<Vec<HttpHeader>, Error> {
    values
        .iter()
        .map(|value| parse_single_header(value))
        .collect()
}

fn parse_single_header(value: &str) -> Result<HttpHeader, Error> {
    let Some((name, header_value)) = value.split_once(':') else {
        return Err(Error::ConfigError(format!(
            "invalid header '{}', expected NAME:VALUE",
            value
        )));
    };

    let name = name.trim();
    let header_value = header_value.trim();
    if name.is_empty() || header_value.is_empty() {
        return Err(Error::ConfigError(format!(
            "invalid header '{}', expected non-empty NAME and VALUE",
            value
        )));
    }

    Ok(HttpHeader {
        name: name.to_string(),
        value: header_value.to_string(),
    })
}

fn parse_basic_auth(value: &str) -> Result<HttpAuth, Error> {
    let (username, password) = match value.split_once(':') {
        Some((username, password)) => (
            username.trim().to_string(),
            Some(password.trim().to_string()).filter(|inner| !inner.is_empty()),
        ),
        None => (value.trim().to_string(), None),
    };

    if username.is_empty() {
        return Err(Error::ConfigError(
            "basic auth expects USER[:PASS]".to_string(),
        ));
    }

    Ok(HttpAuth::Basic { username, password })
}

fn prepare_urls(urls: &[String], shuffle_tasks: bool) -> Vec<String> {
    let mut urls = urls.to_vec();
    if shuffle_tasks {
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
        Command::List(filter) => list_messages(manager, filter, false).await,
        Command::History(filter) => list_messages(manager, filter, true).await,
        Command::Show(task_id) => show_task_messages(manager, task_id).await,
        Command::Pause(target) => apply_task_command(manager, target, TaskControl::Pause).await,
        Command::Resume(target) => apply_task_command(manager, target, TaskControl::Resume).await,
        Command::Retry(target) => apply_task_command(manager, target, TaskControl::Retry).await,
        Command::Cancel(target) => apply_task_command(manager, target, TaskControl::Cancel).await,
        Command::Delete(target) => apply_task_command(manager, target, TaskControl::Delete).await,
        Command::SetRateLimit(limit_kbps) => {
            manager.set_rate_limit_kib_per_sec(limit_kbps).await;
            vec![format!(
                "Global rate limit set to {}",
                manager
                    .current_rate_limit_kib_per_sec()
                    .map(|value| format!("{value} KiB/s"))
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
    tasks.sort_by_key(|task| task.id());

    if tasks.is_empty() {
        return vec!["No matching tasks".into()];
    }

    let mut messages = vec![format!(
        "Rate limit: {}",
        manager
            .current_rate_limit_kib_per_sec()
            .map(|value| format!("{value} KiB/s"))
            .unwrap_or_else(|| "unlimited".to_string())
    )];
    for task in tasks {
        let snapshot = task.snapshot().await;
        messages.push(format_status_line(&snapshot));
    }
    messages
}

async fn list_messages(
    manager: &Arc<Manager>,
    filter: StatusFilter,
    terminal_only: bool,
) -> Vec<String> {
    let mut tasks = manager.get_all_sessions();
    tasks.sort_by_key(|task| task.id());

    let mut lines = Vec::new();
    for task in tasks {
        let snapshot = task.snapshot().await;
        if terminal_only && !matches_status_filter(&snapshot, StatusFilter::Terminal) {
            continue;
        }
        if !matches_status_filter(&snapshot, filter) {
            continue;
        }
        lines.push(format_status_line(&snapshot));
    }

    if lines.is_empty() {
        vec!["No matching tasks".into()]
    } else {
        lines
    }
}

async fn show_task_messages(manager: &Arc<Manager>, task_id: u32) -> Vec<String> {
    let Some(session) = manager.get_session_by_id(task_id) else {
        return vec![format!("Session #{task_id} not found")];
    };
    let snapshot = session.snapshot().await;
    vec![
        format!("Session #{} ({})", snapshot.id, snapshot.trace_id),
        format!("  status: {}", snapshot.status),
        format!(
            "  progress: {} / {} (pieces {}/{})",
            format_bytes(snapshot.downloaded_size),
            format_snapshot_total(snapshot.total_size_known, snapshot.total_size),
            snapshot.completed_pieces,
            snapshot.piece_count
        ),
        format!(
            "  source: {}",
            snapshot
                .primary_source_locator
                .as_deref()
                .unwrap_or(snapshot.locator.as_str())
        ),
        format!("  sources: {}", snapshot.source_count),
        format!(
            "  file: {}",
            snapshot
                .file_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<uninitialized>".into())
        ),
        format!(
            "  avg_speed: {} retries: {} resume_hits: {}/{}",
            format_bytes(snapshot.stats.average_speed_bps),
            snapshot.stats.retry_count,
            snapshot.stats.resume_hits,
            snapshot.stats.resume_attempts
        ),
    ]
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
            .get_all_sessions()
            .into_iter()
            .map(|session| session.id())
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

fn resolve_target_tasks(manager: &Arc<Manager>, target: CommandTarget) -> Vec<Session> {
    match target {
        CommandTarget::All => manager.get_all_sessions(),
        CommandTarget::Tasks(ids) => ids
            .into_iter()
            .filter_map(|task_id| manager.get_session_by_id(task_id))
            .collect(),
    }
}

fn format_status_line(snapshot: &SessionSnapshot) -> String {
    let progress = format_progress(
        snapshot.total_size_known,
        snapshot.downloaded_size,
        snapshot.total_size,
    );
    let piece_label = if snapshot.piece_count > 0 {
        format!(
            " pieces:{}/{}",
            snapshot.completed_pieces, snapshot.piece_count
        )
    } else {
        String::new()
    };

    format!(
        "#{} {} {} {}/{}{} {} {}",
        snapshot.id,
        snapshot.status,
        progress,
        format_bytes(snapshot.downloaded_size),
        format_snapshot_total(snapshot.total_size_known, snapshot.total_size),
        piece_label,
        snapshot.trace_id,
        snapshot.file_name.as_deref().unwrap_or_else(|| {
            snapshot
                .primary_source_locator
                .as_deref()
                .unwrap_or(snapshot.locator.as_str())
        })
    )
}

fn format_progress(total_size_known: bool, downloaded_size: u64, total_size: u64) -> String {
    if !total_size_known || total_size == 0 {
        "-".into()
    } else {
        format!("{:.1}%", downloaded_size as f64 / total_size as f64 * 100.0)
    }
}

fn format_snapshot_total(total_size_known: bool, total_size: u64) -> String {
    if total_size_known {
        format_bytes(total_size)
    } else {
        "unknown".into()
    }
}

fn matches_status_filter(snapshot: &SessionSnapshot, filter: StatusFilter) -> bool {
    match filter {
        StatusFilter::All => true,
        StatusFilter::Pending => snapshot.status == "Pending",
        StatusFilter::Preparing => snapshot.status == "Preparing",
        StatusFilter::Running => snapshot.status == "Running",
        StatusFilter::Paused => snapshot.status == "Paused",
        StatusFilter::Completed => snapshot.status == "Completed",
        StatusFilter::Failed => snapshot.status.starts_with("Failed"),
        StatusFilter::Canceled => snapshot.status == "Canceled",
        StatusFilter::Deleted => snapshot.status == "Deleted",
        StatusFilter::Active => matches!(
            snapshot.status.as_str(),
            "Pending" | "Preparing" | "Running" | "Paused"
        ),
        StatusFilter::Terminal => {
            matches!(
                snapshot.status.as_str(),
                "Completed" | "Canceled" | "Deleted"
            ) || snapshot.status.starts_with("Failed")
        }
    }
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
            .get_all_sessions()
            .into_iter()
            .map(|session| session.id())
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

    let Some(session) = manager.get_session_by_id(task_id) else {
        return format!(
            "Failed to {} session #{task_id}: {err}",
            control.verb_label()
        );
    };
    let snapshot = session.snapshot().await;
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

fn status_hint(control: TaskControl, snapshot: &SessionSnapshot) -> &'static str {
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

#[derive(Serialize)]
struct CompletionSummary {
    snapshots: Vec<SessionSnapshot>,
    completed: usize,
    failed: usize,
    canceled: usize,
    deleted: usize,
    total_downloaded: u64,
    known_total_size: u64,
    has_unknown_total: bool,
    average_speed_bps: u64,
    retry_count: u64,
    resume_attempts: u64,
    resume_hits: u64,
}

async fn build_completion_summary(manager: &Arc<Manager>) -> CompletionSummary {
    let mut tasks = manager.get_all_sessions();
    tasks.sort_by_key(|session| session.id());

    let mut snapshots = Vec::with_capacity(tasks.len());
    let mut completed = 0usize;
    let mut failed = 0usize;
    let mut canceled = 0usize;
    let mut deleted = 0usize;
    let mut total_downloaded = 0u64;
    let mut known_total_size = 0u64;
    let mut has_unknown_total = false;
    let mut average_speed_bps = 0u64;
    let mut retry_count = 0u64;
    let mut resume_attempts = 0u64;
    let mut resume_hits = 0u64;

    for task in tasks {
        let snapshot = task.snapshot().await;
        total_downloaded = total_downloaded.saturating_add(snapshot.downloaded_size);
        if snapshot.total_size_known {
            known_total_size = known_total_size.saturating_add(snapshot.total_size);
        } else {
            has_unknown_total = true;
        }
        average_speed_bps = average_speed_bps.saturating_add(snapshot.stats.average_speed_bps);
        retry_count = retry_count.saturating_add(snapshot.stats.retry_count);
        resume_attempts = resume_attempts.saturating_add(snapshot.stats.resume_attempts);
        resume_hits = resume_hits.saturating_add(snapshot.stats.resume_hits);
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
        known_total_size,
        has_unknown_total,
        average_speed_bps,
        retry_count,
        resume_attempts,
        resume_hits,
    }
}

fn print_completion_summary(summary: &CompletionSummary) {
    println!(
        "Final summary: tasks={} completed={} failed={} canceled={} deleted={} total={} / {} avg_speed={} retries={} resume_hits={}/{}",
        summary.snapshots.len(),
        summary.completed,
        summary.failed,
        summary.canceled,
        summary.deleted,
        format_bytes(summary.total_downloaded),
        format_total_summary(summary.known_total_size, summary.has_unknown_total),
        format_bytes(summary.average_speed_bps),
        summary.retry_count,
        summary.resume_hits,
        summary.resume_attempts,
    );
    for snapshot in &summary.snapshots {
        println!("  {}", format_status_line(snapshot));
        if snapshot.status.starts_with("Failed") {
            println!(
                "    diagnostic: {}",
                diagnostic_path_for(summary, snapshot.id).display()
            );
        }
    }
}

fn print_json_completion_summary(summary: &CompletionSummary) {
    #[derive(Serialize)]
    struct JsonSummary<'a> {
        kind: &'static str,
        summary: &'a CompletionSummary,
    }

    println!(
        "{}",
        serde_json::to_string(&JsonSummary {
            kind: "summary",
            summary,
        })
        .unwrap_or_else(|_| "{\"kind\":\"summary-error\"}".into())
    );
}

fn exit_code_from_summary(summary: &CompletionSummary) -> ExitCode {
    if summary.failed > 0 || summary.canceled > 0 {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

async fn collect_urls(cli: &Cli) -> Result<Vec<String>, Error> {
    let mut urls = cli.urls.clone();

    for path in &cli.urls_file {
        let content = tokio::fs::read_to_string(path).await.map_err(|err| {
            Error::ConfigError(format!("failed to read {}: {err}", path.display()))
        })?;
        urls.extend(
            content
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(ToOwned::to_owned),
        );
    }

    Ok(urls)
}

async fn run_completion_hook(config: &Config, summary: &CompletionSummary) {
    let Some(command) = &config.completion_hook else {
        return;
    };

    let mut shell = if cfg!(target_os = "windows") {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    } else {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    };

    shell
        .env("PARADOWN_TOTAL_TASKS", summary.snapshots.len().to_string())
        .env("PARADOWN_COMPLETED", summary.completed.to_string())
        .env("PARADOWN_FAILED", summary.failed.to_string())
        .env("PARADOWN_CANCELED", summary.canceled.to_string())
        .env("PARADOWN_DELETED", summary.deleted.to_string())
        .env(
            "PARADOWN_TOTAL_DOWNLOADED",
            summary.total_downloaded.to_string(),
        )
        .env("PARADOWN_TOTAL_SIZE", summary.known_total_size.to_string())
        .env(
            "PARADOWN_TOTAL_SIZE_KNOWN",
            (!summary.has_unknown_total).to_string(),
        );

    if let Err(err) = shell.status().await {
        eprintln!("paradown: failed to run completion hook: {err}");
    }
}

fn diagnostic_path_for(summary: &CompletionSummary, task_id: u32) -> PathBuf {
    let download_dir = summary
        .snapshots
        .iter()
        .find(|snapshot| snapshot.id == task_id)
        .and_then(|snapshot| snapshot.file_path.as_ref())
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));

    download_dir
        .join(".paradown")
        .join("diagnostics")
        .join(format!("task-{}.json", task_id))
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

fn format_total_summary(known_total_size: u64, has_unknown_total: bool) -> String {
    match (known_total_size, has_unknown_total) {
        (0, true) => "unknown".into(),
        (_, true) => format!("{} + unknown", format_bytes(known_total_size)),
        (_, false) => format_bytes(known_total_size),
    }
}
