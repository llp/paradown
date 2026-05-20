use clap::{Parser, ValueEnum};
use paradown::download::{
    DownloadSpec, Event, Manager, SessionRequest, SourceDescriptor, SourceSet,
    TorrentEngineHandle, TorrentPeerEndpoint, TorrentSwarmHints,
};
use paradown::{
    Backend, Config, SwarmIndexProviderConfig, TorrentDiscoveryInputKind,
    TorrentSwarmProviderCandidate, TorrentSwarmProviderCandidateKind,
    TorrentSwarmProviderDiagnostic, TorrentSwarmProviderReport, build_swarm_provider_resolver,
    init_logger_with_level,
};
use paradown_libtorrent_engine::LibtorrentRasterbarEngine;
use std::collections::HashSet;
use std::error::Error as StdError;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::time::{Duration, timeout};

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

    #[arg(long, value_name = "SECONDS")]
    timeout_secs: Option<NonZeroU64>,

    #[arg(long, value_name = "INTERFACES")]
    listen_interfaces: Option<String>,

    #[arg(long = "peer", value_name = "HOST:PORT")]
    peers: Vec<String>,

    #[arg(long = "tracker", value_name = "ANNOUNCE_URL")]
    trackers: Vec<String>,

    #[arg(long = "tracker-file", value_name = "FILE")]
    tracker_files: Vec<PathBuf>,

    #[arg(long = "web-seed", value_name = "URL")]
    web_seeds: Vec<String>,

    #[arg(long = "disable-swarm-providers")]
    disable_swarm_providers: bool,

    #[arg(long = "swarm-provider-cache-dir", value_name = "DIR")]
    swarm_provider_cache_dir: Option<PathBuf>,

    #[arg(long = "tracker-list-url", value_name = "URL")]
    tracker_list_urls: Vec<String>,

    #[arg(long = "tracker-list-cache-ttl-secs", value_name = "SECONDS")]
    tracker_list_cache_ttl_secs: Option<NonZeroU64>,

    #[arg(long = "tracker-list-timeout-secs", value_name = "SECONDS")]
    tracker_list_timeout_secs: Option<NonZeroU64>,

    #[arg(long = "index-url-template", value_name = "URL_TEMPLATE")]
    index_url_templates: Vec<String>,

    #[arg(long = "index-query", value_name = "TEXT")]
    index_query: Option<String>,

    #[arg(long = "index-kind", value_enum, default_value_t = DiscoveryInputArg::Auto)]
    index_kind: DiscoveryInputArg,

    #[arg(long = "index-cache-ttl-secs", value_name = "SECONDS")]
    index_cache_ttl_secs: Option<NonZeroU64>,

    #[arg(long = "index-timeout-secs", value_name = "SECONDS")]
    index_timeout_secs: Option<NonZeroU64>,

    #[arg(long = "swarm-max-trackers", value_name = "COUNT")]
    swarm_max_trackers: Option<usize>,

    #[arg(long = "swarm-max-peers", value_name = "COUNT")]
    swarm_max_peers: Option<usize>,

    #[arg(long = "swarm-max-web-seeds", value_name = "COUNT")]
    swarm_max_web_seeds: Option<usize>,

    #[arg(long = "swarm-max-locators", value_name = "COUNT")]
    swarm_max_locators: Option<usize>,

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

    #[arg(long = "discover-file", value_name = "FILE")]
    discover_files: Vec<PathBuf>,

    #[arg(long = "discover-url", value_name = "URL")]
    discover_urls: Vec<String>,

    #[arg(long = "discover-kind", value_enum, default_value_t = DiscoveryInputArg::Auto)]
    discover_kind: DiscoveryInputArg,

    #[arg(value_name = "TORRENT_OR_MAGNET")]
    locators: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DiscoveryInputArg {
    Auto,
    Html,
    Feed,
    Text,
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

    let mut locators = collect_locators(&cli);
    let mut cli_hints = collect_cli_swarm_hints(&cli)?;
    let provider_reports =
        collect_provider_bootstrap_inputs(&config, &mut locators, &mut cli_hints, &cli).await?;
    print_swarm_provider_reports(&provider_reports);
    if locators.is_empty() {
        return Err(
            "provide at least one .torrent path, magnet URI, discovery file, or discovery URL"
                .into(),
        );
    }

    let engine = Arc::new(LibtorrentRasterbarEngine::new(
        config.p2p.libtorrent.clone(),
    )?);
    let manager = Manager::new_with_torrent_engine(config.clone(), engine.clone())?;
    manager.init().await?;

    let event_task = spawn_event_reporter(Arc::clone(&manager));
    let mut task_ids = Vec::with_capacity(locators.len());
    let mut torrent_handles = Vec::with_capacity(locators.len());
    let mut peer_hints = Vec::new();
    let torrent_input_cache = config.download_dir.join(".paradown").join("torrent-inputs");
    for (index, locator) in locators.into_iter().enumerate() {
        let locator = prepare_native_locator(
            &locator,
            &torrent_input_cache,
            index,
            config.connect_timeout_secs,
        )
        .await?;
        let spec = DownloadSpec::parse(locator)?;
        let source_set = source_set_with_hints(&spec, &cli_hints)?;
        let swarm_hints = TorrentSwarmHints::from_spec_and_sources(&spec, &source_set)?;
        for peer in &swarm_hints.peers {
            if !peer_hints.iter().any(|existing| existing == peer) {
                peer_hints.push(peer.clone());
            }
        }
        let task_id = manager
            .add_session(SessionRequest::builder(spec).sources(source_set).build())
            .await?;
        manager.start_task(task_id).await?;
        if let Some(session) = manager.get_session(task_id)
            && let Some(handle) = session.torrent_handle().await
        {
            torrent_handles.push(handle);
        }
        task_ids.push(task_id);
    }
    if !peer_hints.is_empty() && torrent_handles.is_empty() {
        return Err("explicit peers were provided, but no torrent handles are available".into());
    }
    let peer_task = spawn_peer_connector(Arc::clone(&engine), torrent_handles, peer_hints);

    let timed_out = wait_for_completion(&manager, cli.timeout_secs).await?;
    if timed_out {
        if let Some(timeout_secs) = cli.timeout_secs {
            eprintln!(
                "paradown-libtorrent: timed out after {}s; canceling active tasks",
                timeout_secs
            );
        }
        let _ = manager.cancel_all().await;
    }

    if let Some(peer_task) = peer_task {
        peer_task.abort();
        let _ = peer_task.await;
    }
    event_task.abort();
    let _ = event_task.await;

    let mut exit_code = if timed_out {
        ExitCode::from(124)
    } else {
        ExitCode::SUCCESS
    };
    for task_id in task_ids {
        let Some(session) = manager.get_session(task_id) else {
            if !timed_out {
                exit_code = ExitCode::from(1);
            }
            continue;
        };
        let snapshot = session.snapshot().await;
        print_snapshot_summary(&snapshot);
        if snapshot.status != "Completed" && !timed_out {
            exit_code = ExitCode::from(1);
        }
    }

    Ok(exit_code)
}

async fn prepare_native_locator(
    locator: &str,
    cache_dir: &std::path::Path,
    index: usize,
    timeout_secs: u64,
) -> Result<String> {
    if !is_remote_torrent_locator(locator) {
        return Ok(locator.to_string());
    }

    tokio::fs::create_dir_all(cache_dir).await?;
    let file_name = remote_torrent_cache_name(locator, index);
    let cache_path = cache_dir.join(file_name);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.max(1)))
        .user_agent(concat!("paradown-libtorrent/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let bytes = client
        .get(locator)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    tokio::fs::write(&cache_path, &bytes).await?;
    eprintln!(
        "fetched torrent URL {} -> {}",
        locator,
        cache_path.display()
    );
    Ok(cache_path.to_string_lossy().into_owned())
}

fn is_remote_torrent_locator(locator: &str) -> bool {
    let lowered = locator.to_ascii_lowercase();
    (lowered.starts_with("http://") || lowered.starts_with("https://"))
        && lowered
            .split(['?', '#'])
            .next()
            .is_some_and(|path| path.ends_with(".torrent"))
}

fn remote_torrent_cache_name(locator: &str, index: usize) -> String {
    let base = locator
        .split(['?', '#'])
        .next()
        .and_then(|path| path.rsplit('/').next())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("remote.torrent");
    format!("{index:03}-{}", sanitize_file_name(base))
}

fn sanitize_file_name(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "remote.torrent".into()
    } else {
        sanitized
    }
}

async fn wait_for_completion(
    manager: &Arc<Manager>,
    timeout_secs: Option<NonZeroU64>,
) -> Result<bool> {
    let Some(timeout_secs) = timeout_secs else {
        manager.wait_for_all_tasks().await?;
        return Ok(false);
    };

    match timeout(
        Duration::from_secs(timeout_secs.get()),
        manager.wait_for_all_tasks(),
    )
    .await
    {
        Ok(result) => {
            result?;
            Ok(false)
        }
        Err(_) => Ok(true),
    }
}

fn print_snapshot_summary(snapshot: &paradown::SessionSnapshot) {
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

    if let Some(torrent) = snapshot.torrent.as_ref() {
        eprintln!(
            "#{} swarm {:?} metadata={} peers={} seeds={} down={}/s up={}/s",
            snapshot.id,
            torrent.state,
            torrent.metadata_ready,
            torrent.connected_peers,
            torrent.seeds,
            torrent.download_rate_bps,
            torrent.upload_rate_bps
        );
        for diagnostic in torrent.diagnostics.iter().rev().take(12) {
            eprintln!(
                "#{} diagnostic {}",
                snapshot.id,
                format_diagnostic(diagnostic)
            );
        }
    }
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
    } else if config.p2p.libtorrent.listen_interfaces.is_none() {
        config.p2p.libtorrent.listen_interfaces = Some("0.0.0.0:6881".into());
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
    apply_cli_swarm_provider_config(&mut config, cli);

    config.validate()?;
    Ok(config)
}

fn apply_cli_swarm_provider_config(config: &mut Config, cli: &Cli) {
    if cli.disable_swarm_providers {
        config.p2p.swarm.enabled = false;
    }
    if let Some(cache_dir) = &cli.swarm_provider_cache_dir {
        config.p2p.swarm.cache_dir = Some(cache_dir.clone());
    }
    config
        .p2p
        .swarm
        .static_trackers
        .extend(cli.trackers.iter().cloned());
    config
        .p2p
        .swarm
        .static_tracker_files
        .extend(cli.tracker_files.iter().cloned());
    config
        .p2p
        .swarm
        .static_peers
        .extend(cli.peers.iter().cloned());
    config
        .p2p
        .swarm
        .static_web_seeds
        .extend(cli.web_seeds.iter().cloned());
    config
        .p2p
        .swarm
        .tracker_list_urls
        .extend(cli.tracker_list_urls.iter().cloned());
    if let Some(ttl) = cli.tracker_list_cache_ttl_secs {
        config.p2p.swarm.tracker_list_cache_ttl_secs = ttl.get();
    }
    if let Some(timeout) = cli.tracker_list_timeout_secs {
        config.p2p.swarm.tracker_list_timeout_secs = timeout.get();
    }
    for (index, url_template) in cli.index_url_templates.iter().enumerate() {
        config
            .p2p
            .swarm
            .index_providers
            .push(SwarmIndexProviderConfig {
                name: format!("cli-index-{}", index + 1),
                url_template: url_template.clone(),
                input_kind: cli.index_kind.into(),
                cache_ttl_secs: cli
                    .index_cache_ttl_secs
                    .map(NonZeroU64::get)
                    .unwrap_or(config.p2p.swarm.tracker_list_cache_ttl_secs),
                timeout_secs: cli
                    .index_timeout_secs
                    .map(NonZeroU64::get)
                    .unwrap_or(config.p2p.swarm.tracker_list_timeout_secs),
            });
    }
    config
        .p2p
        .swarm
        .discovery_files
        .extend(cli.discover_files.iter().cloned());
    config
        .p2p
        .swarm
        .discovery_urls
        .extend(cli.discover_urls.iter().cloned());
    config.p2p.swarm.discovery_input_kind = cli.discover_kind.into();
    if let Some(limit) = cli.swarm_max_trackers {
        config.p2p.swarm.limits.max_trackers = limit;
    }
    if let Some(limit) = cli.swarm_max_peers {
        config.p2p.swarm.limits.max_peers = limit;
    }
    if let Some(limit) = cli.swarm_max_web_seeds {
        config.p2p.swarm.limits.max_web_seeds = limit;
    }
    if let Some(limit) = cli.swarm_max_locators {
        config.p2p.swarm.limits.max_locators = limit;
    }
}

fn collect_locators(cli: &Cli) -> Vec<String> {
    cli.urls
        .iter()
        .chain(cli.locators.iter())
        .cloned()
        .collect()
}

async fn collect_provider_bootstrap_inputs(
    config: &Config,
    locators: &mut Vec<String>,
    hints: &mut TorrentSwarmHints,
    cli: &Cli,
) -> Result<Vec<TorrentSwarmProviderReport>> {
    let mut bootstrap_config = config.clone();
    if cli.index_query.is_none() && !locators.is_empty() {
        bootstrap_config.p2p.swarm.index_providers.clear();
    }
    let Some(resolver) =
        build_swarm_provider_resolver(&bootstrap_config.p2p.swarm, &bootstrap_config.download_dir)?
    else {
        return Ok(Vec::new());
    };
    let resolution = resolver
        .resolve(
            DownloadSpec::Metadata {
                display_name: cli.index_query.clone(),
                info_hash: None,
            },
            hints.clone(),
        )
        .await;
    for candidate in &resolution.candidates {
        apply_provider_bootstrap_candidate(candidate, locators);
    }
    *hints = resolution.hints;
    print_swarm_provider_diagnostics(&resolution.diagnostics);
    Ok(resolution.reports)
}

fn apply_provider_bootstrap_candidate(
    candidate: &TorrentSwarmProviderCandidate,
    locators: &mut Vec<String>,
) {
    if matches!(
        candidate.kind,
        TorrentSwarmProviderCandidateKind::Magnet | TorrentSwarmProviderCandidateKind::TorrentFile
    ) && !locators.iter().any(|locator| locator == &candidate.locator)
    {
        locators.push(candidate.locator.clone());
    }
}

fn print_swarm_provider_reports(reports: &[TorrentSwarmProviderReport]) {
    for report in reports {
        eprintln!(
            "swarm provider {} candidates from {}",
            report.candidates.len(),
            report.provider
        );
        for candidate in &report.candidates {
            if let Some(display_name) = candidate.display_name.as_deref() {
                eprintln!(
                    "  {} {} ({})",
                    provider_kind_label(candidate.kind),
                    candidate.locator,
                    display_name
                );
            } else {
                eprintln!(
                    "  {} {}",
                    provider_kind_label(candidate.kind),
                    candidate.locator
                );
            }
        }
    }
}

fn print_swarm_provider_diagnostics(diagnostics: &[TorrentSwarmProviderDiagnostic]) {
    for diagnostic in diagnostics {
        eprintln!(
            "swarm provider {} {:?}: {}{}",
            diagnostic.provider,
            diagnostic.severity,
            diagnostic.message,
            diagnostic
                .source
                .as_deref()
                .map(|source| format!(" source={source}"))
                .unwrap_or_default()
        );
    }
}

fn provider_kind_label(kind: TorrentSwarmProviderCandidateKind) -> &'static str {
    match kind {
        TorrentSwarmProviderCandidateKind::Magnet => "magnet",
        TorrentSwarmProviderCandidateKind::TorrentFile => "torrent",
        TorrentSwarmProviderCandidateKind::Tracker => "tracker",
        TorrentSwarmProviderCandidateKind::Peer => "peer",
        TorrentSwarmProviderCandidateKind::WebSeed => "web-seed",
    }
}

fn collect_cli_swarm_hints(cli: &Cli) -> Result<TorrentSwarmHints> {
    let mut hints = TorrentSwarmHints::default();
    for tracker in &cli.trackers {
        hints.add_tracker(tracker.clone());
    }
    for tracker in read_tracker_files(&cli.tracker_files)? {
        hints.add_tracker(tracker);
    }
    for peer in &cli.peers {
        hints.add_peer(TorrentPeerEndpoint::parse(peer)?);
    }
    for web_seed in &cli.web_seeds {
        hints.add_web_seed(web_seed.clone());
    }
    Ok(hints)
}

fn read_tracker_files(paths: &[PathBuf]) -> Result<Vec<String>> {
    let mut trackers = Vec::new();
    for path in paths {
        let contents = std::fs::read_to_string(path)
            .map_err(|err| format!("failed to read tracker file {}: {err}", path.display()))?;
        for line in contents.lines() {
            let line = line.trim();
            if !line.is_empty() && !line.starts_with('#') {
                trackers.push(line.to_string());
            }
        }
    }
    Ok(trackers)
}

fn source_set_with_hints(spec: &DownloadSpec, hints: &TorrentSwarmHints) -> Result<SourceSet> {
    let mut source_set = SourceSet::for_spec(spec, None);
    for tracker in &hints.trackers {
        source_set.push_unique(SourceDescriptor::tracker(tracker.clone()));
    }
    for peer in &hints.peers {
        source_set.push_unique(SourceDescriptor::peer(peer.to_string()));
    }
    for web_seed in &hints.web_seeds {
        source_set.push_unique(SourceDescriptor::web_seed(web_seed.clone()));
    }
    Ok(source_set)
}

fn spawn_peer_connector(
    engine: Arc<LibtorrentRasterbarEngine>,
    handles: Vec<TorrentEngineHandle>,
    peers: Vec<TorrentPeerEndpoint>,
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
                Event::TorrentDiagnostic { id, diagnostic } => {
                    eprintln!("#{id} torrent {}", format_diagnostic(&diagnostic));
                }
                Event::Cancel(id) => eprintln!("#{id} canceled"),
                Event::Delete(id) => eprintln!("#{id} deleted"),
            }
        }
    })
}

fn format_diagnostic(diagnostic: &paradown::TorrentDiagnosticEvent) -> String {
    format!(
        "{:?}/{:?}: {}{}{}{}",
        diagnostic.scope,
        diagnostic.severity,
        diagnostic.message,
        diagnostic
            .url
            .as_deref()
            .map(|url| format!(" url={url}"))
            .unwrap_or_default(),
        diagnostic
            .endpoint
            .as_deref()
            .map(|endpoint| format!(" endpoint={endpoint}"))
            .unwrap_or_default(),
        diagnostic
            .peers
            .map(|peers| format!(" peers={peers}"))
            .unwrap_or_default()
    )
}

impl From<DiscoveryInputArg> for TorrentDiscoveryInputKind {
    fn from(value: DiscoveryInputArg) -> Self {
        match value {
            DiscoveryInputArg::Auto => Self::Auto,
            DiscoveryInputArg::Html => Self::Html,
            DiscoveryInputArg::Feed => Self::Feed,
            DiscoveryInputArg::Text => Self::Text,
        }
    }
}
