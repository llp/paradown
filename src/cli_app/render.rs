use paradown::download::{Event, Manager, SessionSnapshot};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Duration, MissedTickBehavior, interval};

const MAX_MESSAGES: usize = 8;
const BAR_WIDTH: usize = 24;

pub(crate) type DashboardMessageTx = mpsc::UnboundedSender<String>;

pub(crate) struct DashboardHandle {
    message_tx: DashboardMessageTx,
    stop_tx: watch::Sender<bool>,
    render_task: JoinHandle<()>,
}

pub(crate) struct PlainTextHandle {
    stop_tx: watch::Sender<bool>,
    render_task: JoinHandle<()>,
}

pub(crate) struct JsonHandle {
    stop_tx: watch::Sender<bool>,
    render_task: JoinHandle<()>,
}

impl PlainTextHandle {
    pub(crate) fn spawn(manager: Arc<Manager>) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let render_task = tokio::spawn(async move {
            let mut runner = PlainTextRunner::new(manager);
            runner.run(stop_rx).await;
        });

        Self {
            stop_tx,
            render_task,
        }
    }

    pub(crate) async fn shutdown(self) {
        let _ = self.stop_tx.send(true);
        let _ = self.render_task.await;
    }
}

impl JsonHandle {
    pub(crate) fn spawn(manager: Arc<Manager>) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let render_task = tokio::spawn(async move {
            let mut runner = JsonRunner::new(manager);
            runner.run(stop_rx).await;
        });

        Self {
            stop_tx,
            render_task,
        }
    }

    pub(crate) async fn shutdown(self) {
        let _ = self.stop_tx.send(true);
        let _ = self.render_task.await;
    }
}

impl DashboardHandle {
    pub(crate) fn spawn(manager: Arc<Manager>, interactive: bool) -> Self {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let (stop_tx, stop_rx) = watch::channel(false);
        let render_task = tokio::spawn(async move {
            let mut runner = DashboardRunner::new(manager, interactive, message_rx);
            runner.run(stop_rx).await;
        });

        Self {
            message_tx,
            stop_tx,
            render_task,
        }
    }

    pub(crate) fn message_tx(&self) -> DashboardMessageTx {
        self.message_tx.clone()
    }

    pub(crate) async fn shutdown(self) {
        let _ = self.stop_tx.send(true);
        let _ = self.render_task.await;
    }
}

struct DashboardRunner {
    manager: Arc<Manager>,
    interactive: bool,
    message_rx: mpsc::UnboundedReceiver<String>,
    messages: VecDeque<String>,
    speed_state: HashMap<u32, SpeedSample>,
}

struct PlainTextRunner {
    manager: Arc<Manager>,
    speed_state: HashMap<u32, SpeedSample>,
    last_signature: Option<String>,
}

struct JsonRunner {
    manager: Arc<Manager>,
    speed_state: HashMap<u32, SpeedSample>,
    last_signature: Option<String>,
}

impl DashboardRunner {
    fn new(
        manager: Arc<Manager>,
        interactive: bool,
        message_rx: mpsc::UnboundedReceiver<String>,
    ) -> Self {
        let mut messages = VecDeque::new();
        if interactive {
            messages.push_back(
                "Interactive mode enabled. Type commands and press Enter; the dashboard will keep refreshing.".into(),
            );
        }

        Self {
            manager,
            interactive,
            message_rx,
            messages,
            speed_state: HashMap::new(),
        }
    }

    async fn run(&mut self, mut stop_rx: watch::Receiver<bool>) {
        let _guard = TerminalGuard::activate();
        let mut event_rx = self.manager.subscribe_events();
        let mut ticker = interval(Duration::from_millis(250));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let _ = self.render(false).await;

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let _ = self.render(false).await;
                }
                event = event_rx.recv() => {
                    self.handle_event(event);
                    let _ = self.render(false).await;
                }
                Some(message) = self.message_rx.recv() => {
                    self.push_message(message);
                    let _ = self.render(false).await;
                }
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        self.push_message("All tasks reached terminal states.".into());
                        let _ = self.render(true).await;
                        break;
                    }
                }
            }
        }
    }

    fn handle_event(&mut self, event: Result<Event, broadcast::error::RecvError>) {
        match event {
            Ok(Event::Start(id)) => self.push_message(format!("Task #{id} started")),
            Ok(Event::Pause(id)) => self.push_message(format!("Task #{id} paused")),
            Ok(Event::Complete(id)) => self.push_message(format!("Task #{id} completed")),
            Ok(Event::Cancel(id)) => self.push_message(format!("Task #{id} canceled")),
            Ok(Event::Delete(id)) => self.push_message(format!("Task #{id} deleted")),
            Ok(Event::Error(id, err)) => self.push_message(format!("Task #{id} failed: {err}")),
            Ok(Event::Pending(_) | Event::Preparing(_) | Event::Progress { .. }) => {}
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                self.push_message(format!(
                    "Dashboard skipped {skipped} events because rendering lagged behind",
                ));
            }
            Err(broadcast::error::RecvError::Closed) => {
                self.push_message("Event stream closed".into());
            }
        }
    }

    fn push_message(&mut self, message: String) {
        if message.trim().is_empty() {
            return;
        }

        while self.messages.len() >= MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(message);
    }

    async fn render(&mut self, final_frame: bool) -> std::io::Result<()> {
        let snapshots = collect_snapshots(&self.manager).await;
        let render_state = self.sample_render_state(&snapshots);
        let frame = render_dashboard(
            &snapshots,
            &render_state,
            &self.messages,
            self.manager.current_rate_limit_kib_per_sec(),
            self.interactive,
            final_frame,
        );

        let mut stdout = std::io::stdout();
        stdout.write_all(frame.as_bytes())?;
        stdout.flush()
    }

    fn sample_render_state(&mut self, snapshots: &[SessionSnapshot]) -> RenderState {
        let now = Instant::now();
        let mut task_speeds = HashMap::with_capacity(snapshots.len());
        let mut active_ids = Vec::with_capacity(snapshots.len());
        let mut total_speed = 0f64;

        for snapshot in snapshots {
            let previous = self.speed_state.get(&snapshot.id).cloned();
            let speed_bps = previous
                .and_then(|previous| {
                    let elapsed = now.duration_since(previous.observed_at).as_secs_f64();
                    if elapsed <= f64::EPSILON {
                        return None;
                    }

                    let delta = snapshot
                        .downloaded_size
                        .saturating_sub(previous.downloaded_size);
                    let sample_speed = delta as f64 / elapsed;
                    let smoothed = if previous.smoothed_bps > 0.0 {
                        previous.smoothed_bps * 0.6 + sample_speed * 0.4
                    } else {
                        sample_speed
                    };
                    Some(smoothed)
                })
                .unwrap_or(0.0);

            self.speed_state.insert(
                snapshot.id,
                SpeedSample {
                    downloaded_size: snapshot.downloaded_size,
                    observed_at: now,
                    smoothed_bps: speed_bps,
                },
            );
            active_ids.push(snapshot.id);
            task_speeds.insert(snapshot.id, speed_bps);
            total_speed += speed_bps;
        }

        self.speed_state
            .retain(|task_id, _| active_ids.contains(task_id));

        RenderState {
            task_speeds,
            total_speed_bps: total_speed,
        }
    }
}

impl PlainTextRunner {
    fn new(manager: Arc<Manager>) -> Self {
        Self {
            manager,
            speed_state: HashMap::new(),
            last_signature: None,
        }
    }

    async fn run(&mut self, mut stop_rx: watch::Receiver<bool>) {
        let mut event_rx = self.manager.subscribe_events();
        let mut ticker = interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let _ = self.render(false).await;

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let _ = self.render(false).await;
                }
                event = event_rx.recv() => {
                    if self.handle_event(event) {
                        let _ = self.render(false).await;
                    }
                }
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        let _ = self.render(true).await;
                        break;
                    }
                }
            }
        }
    }

    fn handle_event(&self, event: Result<Event, broadcast::error::RecvError>) -> bool {
        matches!(
            event,
            Ok(Event::Start(_))
                | Ok(Event::Pause(_))
                | Ok(Event::Complete(_))
                | Ok(Event::Cancel(_))
                | Ok(Event::Delete(_))
                | Ok(Event::Error(_, _))
                | Err(broadcast::error::RecvError::Lagged(_))
                | Err(broadcast::error::RecvError::Closed)
        )
    }

    async fn render(&mut self, final_frame: bool) -> std::io::Result<()> {
        let snapshots = collect_snapshots(&self.manager).await;
        let render_state = self.sample_render_state(&snapshots);
        let frame = render_plain_progress(
            &snapshots,
            &render_state,
            self.manager.current_rate_limit_kib_per_sec(),
            final_frame,
        );
        let signature = plain_signature(
            &snapshots,
            self.manager.current_rate_limit_kib_per_sec(),
            final_frame,
        );

        if !final_frame && self.last_signature.as_deref() == Some(signature.as_str()) {
            return Ok(());
        }
        self.last_signature = Some(signature);

        let mut stdout = std::io::stdout();
        stdout.write_all(frame.as_bytes())?;
        stdout.flush()
    }

    fn sample_render_state(&mut self, snapshots: &[SessionSnapshot]) -> RenderState {
        let now = Instant::now();
        let mut task_speeds = HashMap::with_capacity(snapshots.len());
        let mut active_ids = Vec::with_capacity(snapshots.len());
        let mut total_speed = 0f64;

        for snapshot in snapshots {
            let previous = self.speed_state.get(&snapshot.id).cloned();
            let speed_bps = previous
                .and_then(|previous| {
                    let elapsed = now.duration_since(previous.observed_at).as_secs_f64();
                    if elapsed <= f64::EPSILON {
                        return None;
                    }

                    let delta = snapshot
                        .downloaded_size
                        .saturating_sub(previous.downloaded_size);
                    Some(delta as f64 / elapsed)
                })
                .unwrap_or(0.0);

            self.speed_state.insert(
                snapshot.id,
                SpeedSample {
                    downloaded_size: snapshot.downloaded_size,
                    observed_at: now,
                    smoothed_bps: speed_bps,
                },
            );
            active_ids.push(snapshot.id);
            task_speeds.insert(snapshot.id, speed_bps);
            total_speed += speed_bps;
        }

        self.speed_state
            .retain(|task_id, _| active_ids.contains(task_id));

        RenderState {
            task_speeds,
            total_speed_bps: total_speed,
        }
    }
}

impl JsonRunner {
    fn new(manager: Arc<Manager>) -> Self {
        Self {
            manager,
            speed_state: HashMap::new(),
            last_signature: None,
        }
    }

    async fn run(&mut self, mut stop_rx: watch::Receiver<bool>) {
        let mut event_rx = self.manager.subscribe_events();
        let mut ticker = interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let _ = self.render(false).await;

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let _ = self.render(false).await;
                }
                event = event_rx.recv() => {
                    if self.handle_event(event) {
                        let _ = self.render(false).await;
                    }
                }
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        let _ = self.render(true).await;
                        break;
                    }
                }
            }
        }
    }

    fn handle_event(&self, event: Result<Event, broadcast::error::RecvError>) -> bool {
        matches!(
            event,
            Ok(Event::Start(_))
                | Ok(Event::Pause(_))
                | Ok(Event::Complete(_))
                | Ok(Event::Cancel(_))
                | Ok(Event::Delete(_))
                | Ok(Event::Error(_, _))
                | Err(broadcast::error::RecvError::Lagged(_))
                | Err(broadcast::error::RecvError::Closed)
        )
    }

    async fn render(&mut self, final_frame: bool) -> std::io::Result<()> {
        let snapshots = collect_snapshots(&self.manager).await;
        let render_state = self.sample_render_state(&snapshots);
        let signature = plain_signature(
            &snapshots,
            self.manager.current_rate_limit_kib_per_sec(),
            final_frame,
        );

        if !final_frame && self.last_signature.as_deref() == Some(signature.as_str()) {
            return Ok(());
        }
        self.last_signature = Some(signature);

        let frame = render_json_progress(
            &snapshots,
            &render_state,
            self.manager.current_rate_limit_kib_per_sec(),
            final_frame,
        );
        let mut stdout = std::io::stdout();
        stdout.write_all(frame.as_bytes())?;
        stdout.flush()
    }

    fn sample_render_state(&mut self, snapshots: &[SessionSnapshot]) -> RenderState {
        let now = Instant::now();
        let mut task_speeds = HashMap::with_capacity(snapshots.len());
        let mut active_ids = Vec::with_capacity(snapshots.len());
        let mut total_speed = 0f64;

        for snapshot in snapshots {
            let previous = self.speed_state.get(&snapshot.id).cloned();
            let speed_bps = previous
                .and_then(|previous| {
                    let elapsed = now.duration_since(previous.observed_at).as_secs_f64();
                    if elapsed <= f64::EPSILON {
                        return None;
                    }

                    let delta = snapshot
                        .downloaded_size
                        .saturating_sub(previous.downloaded_size);
                    Some(delta as f64 / elapsed)
                })
                .unwrap_or(0.0);

            self.speed_state.insert(
                snapshot.id,
                SpeedSample {
                    downloaded_size: snapshot.downloaded_size,
                    observed_at: now,
                    smoothed_bps: speed_bps,
                },
            );
            active_ids.push(snapshot.id);
            task_speeds.insert(snapshot.id, speed_bps);
            total_speed += speed_bps;
        }

        self.speed_state
            .retain(|task_id, _| active_ids.contains(task_id));

        RenderState {
            task_speeds,
            total_speed_bps: total_speed,
        }
    }
}

#[derive(Clone)]
struct SpeedSample {
    downloaded_size: u64,
    observed_at: Instant,
    smoothed_bps: f64,
}

struct RenderState {
    task_speeds: HashMap<u32, f64>,
    total_speed_bps: f64,
}

async fn collect_snapshots(manager: &Manager) -> Vec<SessionSnapshot> {
    let mut sessions = manager.get_all_sessions();
    sessions.sort_by_key(|session| session.id());

    let mut snapshots = Vec::with_capacity(sessions.len());
    for session in sessions {
        snapshots.push(session.snapshot().await);
    }
    snapshots
}

fn render_dashboard(
    snapshots: &[SessionSnapshot],
    render_state: &RenderState,
    messages: &VecDeque<String>,
    rate_limit_kib_per_sec: Option<u64>,
    interactive: bool,
    final_frame: bool,
) -> String {
    let mut output = String::new();
    output.push_str("\x1b[2J\x1b[H");
    output.push_str("paradown download dashboard\n");

    let status_summary = summarize_statuses(snapshots);
    let totals = aggregate_totals(snapshots);

    output.push_str(&format!(
        "Tasks:{} running:{} preparing:{} paused:{} pending:{} finished:{}  Total:{} / {}  Speed:{}  Rate:{}\n",
        snapshots.len(),
        status_summary.running,
        status_summary.preparing,
        status_summary.paused,
        status_summary.pending,
        status_summary.terminal,
        format_bytes(totals.total_downloaded),
        format_total_label(totals.known_total_size, totals.has_unknown_total),
        format_speed(render_state.total_speed_bps),
        format_rate_limit(rate_limit_kib_per_sec),
    ));

    if interactive {
        output.push_str(
            "Commands: help | status [all|id ...] | pause [all|id ...] | resume [all|id ...] | retry [all|id ...] | cancel [all|id ...] | delete [all|id ...] | limit <kib|off>\n",
        );
    } else {
        output
            .push_str("Run with --interactive to control tasks while the dashboard is visible.\n");
    }

    if final_frame {
        output.push_str("State: all tasks are in terminal states.\n");
    } else {
        output.push_str("State: live refresh every 250ms.\n");
    }

    output.push_str("----------------------------------------------------------------------------------------------------\n");

    if snapshots.is_empty() {
        output.push_str("No tasks\n");
    } else {
        for snapshot in snapshots {
            let speed_bps = render_state
                .task_speeds
                .get(&snapshot.id)
                .copied()
                .unwrap_or_default();
            output.push_str(&render_task_line(snapshot, speed_bps));
            output.push('\n');
        }
    }

    output.push_str("\nMessages:\n");
    if messages.is_empty() {
        output.push_str("  (no recent messages)\n");
    } else {
        for message in messages {
            output.push_str("  ");
            output.push_str(message);
            output.push('\n');
        }
    }

    output
}

fn render_plain_progress(
    snapshots: &[SessionSnapshot],
    render_state: &RenderState,
    rate_limit_kib_per_sec: Option<u64>,
    final_frame: bool,
) -> String {
    let mut output = String::new();
    let status_summary = summarize_statuses(snapshots);
    let totals = aggregate_totals(snapshots);

    output.push_str(&format!(
        "{} tasks={} running={} preparing={} paused={} pending={} finished={} total={} / {} speed={} rate={}\n",
        if final_frame { "final" } else { "progress" },
        snapshots.len(),
        status_summary.running,
        status_summary.preparing,
        status_summary.paused,
        status_summary.pending,
        status_summary.terminal,
        format_bytes(totals.total_downloaded),
        format_total_label(totals.known_total_size, totals.has_unknown_total),
        format_speed(render_state.total_speed_bps),
        format_rate_limit(rate_limit_kib_per_sec),
    ));

    for snapshot in snapshots {
        let speed_bps = render_state
            .task_speeds
            .get(&snapshot.id)
            .copied()
            .unwrap_or_default();
        output.push_str(&format!(
            "  #{} {:<10} {:>6} {:>10} / {:<12} {:>10} {}\n",
            snapshot.id,
            snapshot.status,
            format_progress_percent(snapshot),
            format_bytes(snapshot.downloaded_size),
            format_snapshot_total(snapshot),
            format_speed(speed_bps),
            snapshot.file_name.as_deref().unwrap_or_else(|| snapshot
                .primary_source_locator
                .as_deref()
                .unwrap_or(snapshot.locator.as_str())),
        ));
    }
    output.push('\n');
    output
}

fn render_json_progress(
    snapshots: &[SessionSnapshot],
    render_state: &RenderState,
    rate_limit_kib_per_sec: Option<u64>,
    final_frame: bool,
) -> String {
    #[derive(Serialize)]
    struct JsonTaskProgress {
        #[serde(flatten)]
        snapshot: SessionSnapshot,
        speed_bps: u64,
    }

    #[derive(Serialize)]
    struct JsonProgressFrame {
        kind: &'static str,
        rate_limit_kib_per_sec: Option<u64>,
        total_speed_bps: u64,
        tasks: Vec<JsonTaskProgress>,
    }

    let frame = JsonProgressFrame {
        kind: if final_frame { "final" } else { "progress" },
        rate_limit_kib_per_sec,
        total_speed_bps: render_state.total_speed_bps.round() as u64,
        tasks: snapshots
            .iter()
            .cloned()
            .map(|snapshot| JsonTaskProgress {
                speed_bps: render_state
                    .task_speeds
                    .get(&snapshot.id)
                    .copied()
                    .unwrap_or_default()
                    .round() as u64,
                snapshot,
            })
            .collect(),
    };

    serde_json::to_string(&frame).unwrap_or_else(|_| "{\"kind\":\"error\"}".to_string()) + "\n"
}

fn plain_signature(
    snapshots: &[SessionSnapshot],
    rate_limit_kib_per_sec: Option<u64>,
    final_frame: bool,
) -> String {
    let mut signature = format!("rate={rate_limit_kib_per_sec:?};final={final_frame};");
    for snapshot in snapshots {
        signature.push_str(&format!(
            "{}:{}:{}:{}:{}:{};",
            snapshot.id,
            snapshot.status,
            snapshot.downloaded_size,
            snapshot.total_size,
            snapshot.total_size_known,
            snapshot.completed_pieces
        ));
    }
    signature
}

fn render_task_line(snapshot: &SessionSnapshot, speed_bps: f64) -> String {
    let percent = progress_ratio(snapshot).unwrap_or_default();
    let bar = progress_bar(percent, BAR_WIDTH);
    let label = snapshot.file_name.clone().unwrap_or_else(|| {
        snapshot
            .primary_source_locator
            .clone()
            .unwrap_or_else(|| snapshot.locator.clone())
    });
    let label = truncate_middle(&label, 30);
    let piece_label = if snapshot.piece_count > 0 {
        format!(
            " {}/{} pieces",
            snapshot.completed_pieces, snapshot.piece_count
        )
    } else {
        String::new()
    };

    format!(
        "#{:<3} {:<10} {} {:>6} {:>10} / {:<12} {:>10}{}  {}",
        snapshot.id,
        snapshot.status,
        bar,
        format_progress_percent(snapshot),
        format_bytes(snapshot.downloaded_size),
        format_snapshot_total(snapshot),
        format_speed(speed_bps),
        piece_label,
        label,
    )
}

fn progress_ratio(snapshot: &SessionSnapshot) -> Option<f64> {
    if !snapshot.total_size_known || snapshot.total_size == 0 {
        None
    } else {
        Some(snapshot.downloaded_size as f64 / snapshot.total_size as f64)
    }
}

fn format_progress_percent(snapshot: &SessionSnapshot) -> String {
    progress_ratio(snapshot)
        .map(|ratio| format!("{:.1}%", ratio * 100.0))
        .unwrap_or_else(|| "-".to_string())
}

fn format_snapshot_total(snapshot: &SessionSnapshot) -> String {
    if snapshot.total_size_known {
        format_bytes(snapshot.total_size)
    } else {
        "unknown".to_string()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AggregateTotals {
    total_downloaded: u64,
    known_total_size: u64,
    has_unknown_total: bool,
}

fn aggregate_totals(snapshots: &[SessionSnapshot]) -> AggregateTotals {
    let mut totals = AggregateTotals::default();
    for snapshot in snapshots {
        totals.total_downloaded = totals
            .total_downloaded
            .saturating_add(snapshot.downloaded_size);
        if snapshot.total_size_known {
            totals.known_total_size = totals.known_total_size.saturating_add(snapshot.total_size);
        } else {
            totals.has_unknown_total = true;
        }
    }
    totals
}

fn format_total_label(known_total_size: u64, has_unknown_total: bool) -> String {
    match (known_total_size, has_unknown_total) {
        (0, true) => "unknown".to_string(),
        (_, true) => format!("{} + unknown", format_bytes(known_total_size)),
        (_, false) => format_bytes(known_total_size),
    }
}

fn progress_bar(progress: f64, width: usize) -> String {
    let bounded = progress.clamp(0.0, 1.0);
    let filled = (bounded * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "=".repeat(filled), " ".repeat(empty))
}

fn truncate_middle(value: &str, max_len: usize) -> String {
    if value.chars().count() <= max_len {
        return value.to_string();
    }

    let keep_each_side = max_len.saturating_sub(3) / 2;
    let prefix: String = value.chars().take(keep_each_side).collect();
    let suffix: String = value
        .chars()
        .rev()
        .take(keep_each_side)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

fn format_rate_limit(rate_limit_kib_per_sec: Option<u64>) -> String {
    rate_limit_kib_per_sec
        .map(|value| format!("{value} KiB/s"))
        .unwrap_or_else(|| "unlimited".to_string())
}

fn format_speed(speed_bps: f64) -> String {
    if speed_bps <= 0.0 {
        return "-".into();
    }

    format!("{}/s", format_bytes(speed_bps.round() as u64))
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

#[derive(Default)]
struct StatusSummary {
    pending: usize,
    preparing: usize,
    running: usize,
    paused: usize,
    terminal: usize,
}

fn summarize_statuses(snapshots: &[SessionSnapshot]) -> StatusSummary {
    let mut summary = StatusSummary::default();
    for snapshot in snapshots {
        match snapshot.status.as_str() {
            "Pending" => summary.pending += 1,
            "Preparing" => summary.preparing += 1,
            "Running" => summary.running += 1,
            "Paused" => summary.paused += 1,
            "Completed" | "Canceled" | "Deleted" => summary.terminal += 1,
            status if status.starts_with("Failed") => summary.terminal += 1,
            _ => {}
        }
    }
    summary
}

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn activate() -> Self {
        let active = std::io::stdout().is_terminal();
        if active {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(b"\x1b[?25l");
            let _ = stdout.flush();
        }
        Self { active }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }

        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(b"\x1b[?25h\n");
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AggregateTotals, SessionSnapshot, aggregate_totals, format_bytes, format_progress_percent,
        format_total_label, progress_bar, truncate_middle,
    };
    use paradown::Checksum;
    use paradown::StatsSnapshot;
    use std::path::PathBuf;

    fn test_snapshot(
        total_size_known: bool,
        downloaded_size: u64,
        total_size: u64,
    ) -> SessionSnapshot {
        SessionSnapshot {
            id: 1,
            trace_id: "task-000001".into(),
            locator: "https://example.com/file.bin".into(),
            file_name: Some("file.bin".into()),
            file_path: Some(PathBuf::from("/tmp/file.bin")),
            status: "Running".into(),
            downloaded_size,
            total_size,
            total_size_known,
            source_count: 1,
            primary_source_locator: Some("https://example.com/file.bin".into()),
            source_locators: vec!["https://example.com/file.bin".into()],
            completed_pieces: 0,
            piece_count: 0,
            completed_blocks: 0,
            block_count: 0,
            created_at: None,
            updated_at: None,
            checksums: Vec::<Checksum>::new(),
            stats: StatsSnapshot::default(),
        }
    }

    #[test]
    fn formats_byte_values_human_readably() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KiB");
    }

    #[test]
    fn renders_fixed_width_progress_bar() {
        assert_eq!(progress_bar(0.5, 4), "[==  ]");
    }

    #[test]
    fn truncates_long_labels_in_the_middle() {
        assert_eq!(
            truncate_middle("abcdefghijklmnopqrstuvwxyz", 10),
            "abc...xyz"
        );
    }

    #[test]
    fn formats_unknown_total_progress_without_percentage() {
        let snapshot = test_snapshot(false, 12, 0);
        assert_eq!(format_progress_percent(&snapshot), "-");
    }

    #[test]
    fn aggregates_known_and_unknown_totals_separately() {
        let totals = aggregate_totals(&[test_snapshot(true, 10, 20), test_snapshot(false, 5, 0)]);
        assert_eq!(
            totals,
            AggregateTotals {
                total_downloaded: 15,
                known_total_size: 20,
                has_unknown_total: true,
            }
        );
        assert_eq!(
            format_total_label(totals.known_total_size, totals.has_unknown_total),
            "20 B + unknown"
        );
    }
}
