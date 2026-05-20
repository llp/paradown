use super::{MagnetLink, TorrentPeerEndpoint, TorrentSwarmHints};
use crate::discovery::{
    TorrentDiscoveryCandidate, TorrentDiscoveryInputKind, TorrentDiscoveryKind,
    TorrentDiscoveryOptions, discover_torrent_candidates,
};
use crate::domain::{DownloadSpec, SourceDescriptor};
use crate::error::Error;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const DEFAULT_TRACKER_LIST_CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const DEFAULT_PROVIDER_TIMEOUT_SECS: u64 = 15;
const PROVIDER_USER_AGENT: &str = concat!("paradown/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmProviderConfig {
    #[serde(default = "default_swarm_providers_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,
    #[serde(default = "default_include_magnet_hints")]
    pub include_magnet_hints: bool,
    #[serde(default)]
    pub static_trackers: Vec<String>,
    #[serde(default)]
    pub static_tracker_files: Vec<PathBuf>,
    #[serde(default)]
    pub static_peers: Vec<String>,
    #[serde(default)]
    pub static_web_seeds: Vec<String>,
    #[serde(default)]
    pub tracker_list_urls: Vec<String>,
    #[serde(default = "default_tracker_list_cache_ttl_secs")]
    pub tracker_list_cache_ttl_secs: u64,
    #[serde(default = "default_provider_timeout_secs")]
    pub tracker_list_timeout_secs: u64,
    #[serde(default)]
    pub index_providers: Vec<SwarmIndexProviderConfig>,
    #[serde(default)]
    pub discovery_files: Vec<PathBuf>,
    #[serde(default)]
    pub discovery_urls: Vec<String>,
    #[serde(default)]
    pub discovery_input_kind: TorrentDiscoveryInputKind,
    #[serde(default = "default_provider_timeout_secs")]
    pub discovery_timeout_secs: u64,
    #[serde(default)]
    pub limits: SwarmProviderLimits,
}

impl Default for SwarmProviderConfig {
    fn default() -> Self {
        Self {
            enabled: default_swarm_providers_enabled(),
            cache_dir: None,
            include_magnet_hints: default_include_magnet_hints(),
            static_trackers: Vec::new(),
            static_tracker_files: Vec::new(),
            static_peers: Vec::new(),
            static_web_seeds: Vec::new(),
            tracker_list_urls: Vec::new(),
            tracker_list_cache_ttl_secs: default_tracker_list_cache_ttl_secs(),
            tracker_list_timeout_secs: default_provider_timeout_secs(),
            index_providers: Vec::new(),
            discovery_files: Vec::new(),
            discovery_urls: Vec::new(),
            discovery_input_kind: TorrentDiscoveryInputKind::default(),
            discovery_timeout_secs: default_provider_timeout_secs(),
            limits: SwarmProviderLimits::default(),
        }
    }
}

impl SwarmProviderConfig {
    pub fn resolved_cache_dir(&self, download_dir: &Path) -> PathBuf {
        self.cache_dir
            .clone()
            .unwrap_or_else(|| download_dir.join(".paradown").join("swarm-cache"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwarmIndexProviderConfig {
    pub name: String,
    pub url_template: String,
    #[serde(default)]
    pub input_kind: TorrentDiscoveryInputKind,
    #[serde(default = "default_tracker_list_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
    #[serde(default = "default_provider_timeout_secs")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SwarmProviderLimits {
    #[serde(default = "default_max_trackers")]
    pub max_trackers: usize,
    #[serde(default = "default_max_peers")]
    pub max_peers: usize,
    #[serde(default = "default_max_web_seeds")]
    pub max_web_seeds: usize,
    #[serde(default = "default_max_locators")]
    pub max_locators: usize,
    #[serde(default = "default_max_provider_diagnostics")]
    pub max_diagnostics: usize,
}

impl Default for SwarmProviderLimits {
    fn default() -> Self {
        Self {
            max_trackers: default_max_trackers(),
            max_peers: default_max_peers(),
            max_web_seeds: default_max_web_seeds(),
            max_locators: default_max_locators(),
            max_diagnostics: default_max_provider_diagnostics(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TorrentSwarmProviderCandidateKind {
    Tracker,
    Peer,
    WebSeed,
    Magnet,
    TorrentFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentSwarmProviderCandidate {
    pub kind: TorrentSwarmProviderCandidateKind,
    pub locator: String,
    pub display_name: Option<String>,
    pub provider: String,
    pub source: String,
}

impl TorrentSwarmProviderCandidate {
    fn tracker(provider: &str, locator: impl Into<String>, source: impl Into<String>) -> Self {
        Self::new(
            TorrentSwarmProviderCandidateKind::Tracker,
            locator,
            None,
            provider,
            source,
        )
    }

    fn peer(provider: &str, locator: impl Into<String>, source: impl Into<String>) -> Self {
        Self::new(
            TorrentSwarmProviderCandidateKind::Peer,
            locator,
            None,
            provider,
            source,
        )
    }

    fn web_seed(provider: &str, locator: impl Into<String>, source: impl Into<String>) -> Self {
        Self::new(
            TorrentSwarmProviderCandidateKind::WebSeed,
            locator,
            None,
            provider,
            source,
        )
    }

    fn locator(
        kind: TorrentSwarmProviderCandidateKind,
        locator: impl Into<String>,
        display_name: Option<String>,
        provider: &str,
        source: impl Into<String>,
    ) -> Self {
        Self::new(kind, locator, display_name, provider, source)
    }

    fn new(
        kind: TorrentSwarmProviderCandidateKind,
        locator: impl Into<String>,
        display_name: Option<String>,
        provider: &str,
        source: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            locator: locator.into(),
            display_name,
            provider: provider.into(),
            source: source.into(),
        }
    }

    pub fn source_descriptor(&self) -> Option<SourceDescriptor> {
        match self.kind {
            TorrentSwarmProviderCandidateKind::Tracker => {
                Some(SourceDescriptor::tracker(self.locator.clone()))
            }
            TorrentSwarmProviderCandidateKind::Peer => {
                Some(SourceDescriptor::peer(self.locator.clone()))
            }
            TorrentSwarmProviderCandidateKind::WebSeed => {
                Some(SourceDescriptor::web_seed(self.locator.clone()))
            }
            TorrentSwarmProviderCandidateKind::Magnet
            | TorrentSwarmProviderCandidateKind::TorrentFile => DownloadSpec::parse(&self.locator)
                .ok()
                .map(|spec| SourceDescriptor::from_spec(&spec, None)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TorrentSwarmProviderSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentSwarmProviderDiagnostic {
    pub provider: String,
    pub severity: TorrentSwarmProviderSeverity,
    pub message: String,
    pub source: Option<String>,
}

impl TorrentSwarmProviderDiagnostic {
    fn new(
        provider: &str,
        severity: TorrentSwarmProviderSeverity,
        message: impl Into<String>,
        source: Option<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            severity,
            message: message.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentSwarmProviderReport {
    pub provider: String,
    pub candidates: Vec<TorrentSwarmProviderCandidate>,
    pub diagnostics: Vec<TorrentSwarmProviderDiagnostic>,
}

impl TorrentSwarmProviderReport {
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            candidates: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn push(&mut self, candidate: TorrentSwarmProviderCandidate) {
        if !candidate.locator.trim().is_empty() {
            self.candidates.push(candidate);
        }
    }

    fn diagnostic(
        &mut self,
        severity: TorrentSwarmProviderSeverity,
        message: impl Into<String>,
        source: Option<String>,
    ) {
        self.diagnostics.push(TorrentSwarmProviderDiagnostic::new(
            &self.provider,
            severity,
            message,
            source,
        ));
    }
}

#[derive(Debug, Clone)]
pub struct TorrentSwarmProviderContext {
    pub spec: DownloadSpec,
    pub existing_hints: TorrentSwarmHints,
    pub cache_dir: PathBuf,
}

#[async_trait]
pub trait TorrentSwarmProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn discover(
        &self,
        context: TorrentSwarmProviderContext,
    ) -> Result<TorrentSwarmProviderReport, Error>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentSwarmProviderResolution {
    pub hints: TorrentSwarmHints,
    pub candidates: Vec<TorrentSwarmProviderCandidate>,
    pub diagnostics: Vec<TorrentSwarmProviderDiagnostic>,
    pub reports: Vec<TorrentSwarmProviderReport>,
}

impl TorrentSwarmProviderResolution {
    pub fn empty(hints: TorrentSwarmHints) -> Self {
        Self {
            hints,
            candidates: Vec::new(),
            diagnostics: Vec::new(),
            reports: Vec::new(),
        }
    }
}

pub struct TorrentSwarmProviderResolver {
    providers: Vec<Box<dyn TorrentSwarmProvider>>,
    limits: SwarmProviderLimits,
    cache_dir: PathBuf,
}

impl TorrentSwarmProviderResolver {
    pub fn new(
        providers: Vec<Box<dyn TorrentSwarmProvider>>,
        limits: SwarmProviderLimits,
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            providers,
            limits,
            cache_dir,
        }
    }

    pub async fn resolve(
        &self,
        spec: DownloadSpec,
        initial_hints: TorrentSwarmHints,
    ) -> TorrentSwarmProviderResolution {
        let mut resolution = TorrentSwarmProviderResolution::empty(initial_hints);
        let mut seen = HashSet::new();
        seed_seen_from_hints(&mut seen, &resolution.hints);

        for provider in &self.providers {
            let context = TorrentSwarmProviderContext {
                spec: spec.clone(),
                existing_hints: resolution.hints.clone(),
                cache_dir: self.cache_dir.clone(),
            };
            match provider.discover(context).await {
                Ok(report) => {
                    for diagnostic in &report.diagnostics {
                        push_limited_diagnostic(&mut resolution, diagnostic.clone(), self.limits);
                    }
                    for candidate in report.candidates.iter().cloned() {
                        if !insert_candidate(&mut resolution, &mut seen, candidate, self.limits) {
                            continue;
                        }
                    }
                    resolution.reports.push(report);
                }
                Err(err) => push_limited_diagnostic(
                    &mut resolution,
                    TorrentSwarmProviderDiagnostic::new(
                        provider.name(),
                        TorrentSwarmProviderSeverity::Error,
                        format!("provider failed: {err}"),
                        None,
                    ),
                    self.limits,
                ),
            }
        }

        resolution
    }
}

#[derive(Debug, Clone)]
pub struct MagnetHintProvider;

#[async_trait]
impl TorrentSwarmProvider for MagnetHintProvider {
    fn name(&self) -> &str {
        "magnet-hints"
    }

    async fn discover(
        &self,
        context: TorrentSwarmProviderContext,
    ) -> Result<TorrentSwarmProviderReport, Error> {
        let mut report = TorrentSwarmProviderReport::new(self.name());
        let DownloadSpec::Magnet { uri } = context.spec else {
            return Ok(report);
        };
        let magnet = MagnetLink::parse(&uri)?;
        for tracker in magnet.trackers {
            report.push(TorrentSwarmProviderCandidate::tracker(
                self.name(),
                tracker,
                "magnet:tr",
            ));
        }
        for peer in magnet.peers {
            report.push(TorrentSwarmProviderCandidate::peer(
                self.name(),
                peer.to_string(),
                "magnet:x.pe",
            ));
        }
        for web_seed in magnet.web_seeds {
            report.push(TorrentSwarmProviderCandidate::web_seed(
                self.name(),
                web_seed,
                "magnet:ws",
            ));
        }
        Ok(report)
    }
}

#[derive(Debug, Clone, Default)]
pub struct StaticSwarmProvider {
    pub trackers: Vec<String>,
    pub tracker_files: Vec<PathBuf>,
    pub peers: Vec<String>,
    pub web_seeds: Vec<String>,
}

#[async_trait]
impl TorrentSwarmProvider for StaticSwarmProvider {
    fn name(&self) -> &str {
        "static"
    }

    async fn discover(
        &self,
        _context: TorrentSwarmProviderContext,
    ) -> Result<TorrentSwarmProviderReport, Error> {
        let mut report = TorrentSwarmProviderReport::new(self.name());
        for tracker in &self.trackers {
            report.push(TorrentSwarmProviderCandidate::tracker(
                self.name(),
                tracker.clone(),
                "config:tracker",
            ));
        }
        for path in &self.tracker_files {
            match tokio::fs::read_to_string(path).await {
                Ok(contents) => {
                    for tracker in parse_tracker_lines(&contents) {
                        report.push(TorrentSwarmProviderCandidate::tracker(
                            self.name(),
                            tracker,
                            path.display().to_string(),
                        ));
                    }
                }
                Err(err) => report.diagnostic(
                    TorrentSwarmProviderSeverity::Error,
                    format!("failed to read tracker file {}: {err}", path.display()),
                    Some(path.display().to_string()),
                ),
            }
        }
        for peer in &self.peers {
            match TorrentPeerEndpoint::parse(peer) {
                Ok(peer) => report.push(TorrentSwarmProviderCandidate::peer(
                    self.name(),
                    peer.to_string(),
                    "config:peer",
                )),
                Err(err) => report.diagnostic(
                    TorrentSwarmProviderSeverity::Error,
                    format!("invalid peer '{peer}': {err}"),
                    Some("config:peer".into()),
                ),
            }
        }
        for web_seed in &self.web_seeds {
            report.push(TorrentSwarmProviderCandidate::web_seed(
                self.name(),
                web_seed.clone(),
                "config:web-seed",
            ));
        }
        Ok(report)
    }
}

#[derive(Debug, Clone)]
pub struct TrackerListProvider {
    pub urls: Vec<String>,
    pub cache_ttl: Duration,
    pub timeout: Duration,
    client: reqwest::Client,
}

impl TrackerListProvider {
    pub fn new(urls: Vec<String>, cache_ttl: Duration, timeout: Duration) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(PROVIDER_USER_AGENT)
            .build()
            .map_err(|err| {
                Error::Other(format!(
                    "failed to build tracker provider HTTP client: {err}"
                ))
            })?;
        Ok(Self {
            urls,
            cache_ttl,
            timeout,
            client,
        })
    }

    fn cache_path(&self, cache_dir: &Path, url: &str) -> PathBuf {
        let mut hasher = Sha1::new();
        hasher.update(url.as_bytes());
        let digest = hasher.finalize();
        cache_dir
            .join("tracker-lists")
            .join(format!("{digest:x}.txt"))
    }

    async fn read_source(
        &self,
        url: &str,
        cache_dir: &Path,
        report: &mut TorrentSwarmProviderReport,
    ) -> Option<String> {
        let cache_path = self.cache_path(cache_dir, url);
        if is_fresh_cache(&cache_path, self.cache_ttl).await {
            return tokio::fs::read_to_string(cache_path).await.ok();
        }

        match self.fetch_source(url).await {
            Ok(contents) => {
                if let Some(parent) = cache_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Err(err) = tokio::fs::write(&cache_path, &contents).await {
                    report.diagnostic(
                        TorrentSwarmProviderSeverity::Warning,
                        format!(
                            "failed to write tracker cache {}: {err}",
                            cache_path.display()
                        ),
                        Some(url.into()),
                    );
                }
                Some(contents)
            }
            Err(err) => match tokio::fs::read_to_string(&cache_path).await {
                Ok(contents) => {
                    report.diagnostic(
                        TorrentSwarmProviderSeverity::Warning,
                        format!("using stale tracker cache after fetch failed: {err}"),
                        Some(url.into()),
                    );
                    Some(contents)
                }
                Err(_) => {
                    report.diagnostic(
                        TorrentSwarmProviderSeverity::Error,
                        format!("failed to fetch tracker list: {err}"),
                        Some(url.into()),
                    );
                    None
                }
            },
        }
    }

    async fn fetch_source(&self, url: &str) -> Result<String, Error> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| Error::Other(err.to_string()))?
            .error_for_status()
            .map_err(|err| Error::Other(err.to_string()))?;
        response
            .text()
            .await
            .map_err(|err| Error::Other(err.to_string()))
    }
}

#[async_trait]
impl TorrentSwarmProvider for TrackerListProvider {
    fn name(&self) -> &str {
        "tracker-list"
    }

    async fn discover(
        &self,
        context: TorrentSwarmProviderContext,
    ) -> Result<TorrentSwarmProviderReport, Error> {
        let mut report = TorrentSwarmProviderReport::new(self.name());
        for url in &self.urls {
            let Some(contents) = self.read_source(url, &context.cache_dir, &mut report).await
            else {
                continue;
            };
            for tracker in parse_tracker_lines(&contents) {
                report.push(TorrentSwarmProviderCandidate::tracker(
                    self.name(),
                    tracker,
                    url.clone(),
                ));
            }
        }
        Ok(report)
    }
}

#[derive(Debug, Clone)]
pub struct IndexFeedProvider {
    config: SwarmIndexProviderConfig,
    provider_name: String,
    client: reqwest::Client,
}

impl IndexFeedProvider {
    pub fn new(config: SwarmIndexProviderConfig) -> Result<Self, Error> {
        let mut config = config;
        config.name = config.name.trim().to_string();
        config.url_template = config.url_template.trim().to_string();
        if config.name.is_empty() {
            return Err(Error::Other("index provider name cannot be blank".into()));
        }
        if config.url_template.is_empty() {
            return Err(Error::Other(
                "index provider url_template cannot be blank".into(),
            ));
        }
        if config.cache_ttl_secs == 0 || config.timeout_secs == 0 {
            return Err(Error::Other(
                "index provider cache_ttl_secs and timeout_secs must be greater than 0".into(),
            ));
        }

        let provider_name = format!("index-feed:{}", config.name);
        let client = reqwest::Client::builder()
            .user_agent(PROVIDER_USER_AGENT)
            .build()
            .map_err(|err| {
                Error::Other(format!("failed to build index provider HTTP client: {err}"))
            })?;
        Ok(Self {
            config,
            provider_name,
            client,
        })
    }

    fn cache_path(&self, cache_dir: &Path, url: &str) -> PathBuf {
        let mut hasher = Sha1::new();
        hasher.update(self.provider_name.as_bytes());
        hasher.update(b"\n");
        hasher.update(url.as_bytes());
        let digest = hasher.finalize();
        cache_dir
            .join("index-feeds")
            .join(format!("{digest:x}.txt"))
    }

    async fn read_source(
        &self,
        url: &str,
        cache_dir: &Path,
        report: &mut TorrentSwarmProviderReport,
    ) -> Option<String> {
        let cache_path = self.cache_path(cache_dir, url);
        let cache_ttl = Duration::from_secs(self.config.cache_ttl_secs);
        if is_fresh_cache(&cache_path, cache_ttl).await {
            return tokio::fs::read_to_string(cache_path).await.ok();
        }

        match self.fetch_source(url).await {
            Ok(contents) => {
                if let Some(parent) = cache_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Err(err) = tokio::fs::write(&cache_path, &contents).await {
                    report.diagnostic(
                        TorrentSwarmProviderSeverity::Warning,
                        format!(
                            "failed to write index cache {}: {err}",
                            cache_path.display()
                        ),
                        Some(url.into()),
                    );
                }
                Some(contents)
            }
            Err(err) => match tokio::fs::read_to_string(&cache_path).await {
                Ok(contents) => {
                    report.diagnostic(
                        TorrentSwarmProviderSeverity::Warning,
                        format!("using stale index cache after fetch failed: {err}"),
                        Some(url.into()),
                    );
                    Some(contents)
                }
                Err(_) => {
                    report.diagnostic(
                        TorrentSwarmProviderSeverity::Error,
                        format!("failed to fetch index feed: {err}"),
                        Some(url.into()),
                    );
                    None
                }
            },
        }
    }

    async fn fetch_source(&self, url: &str) -> Result<String, Error> {
        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(self.config.timeout_secs))
            .send()
            .await
            .map_err(|err| Error::Other(err.to_string()))?
            .error_for_status()
            .map_err(|err| Error::Other(err.to_string()))?;
        response
            .text()
            .await
            .map_err(|err| Error::Other(err.to_string()))
    }
}

#[async_trait]
impl TorrentSwarmProvider for IndexFeedProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn discover(
        &self,
        context: TorrentSwarmProviderContext,
    ) -> Result<TorrentSwarmProviderReport, Error> {
        let mut report = TorrentSwarmProviderReport::new(self.name());
        let Some(url) = expand_index_url_template(&self.config.url_template, &context.spec) else {
            report.diagnostic(
                TorrentSwarmProviderSeverity::Warning,
                "index URL template could not be expanded for this torrent spec",
                Some(self.config.url_template.clone()),
            );
            return Ok(report);
        };

        let Some(contents) = self
            .read_source(&url, &context.cache_dir, &mut report)
            .await
        else {
            return Ok(report);
        };
        let options = TorrentDiscoveryOptions {
            input_kind: self.config.input_kind,
            base_url: Some(url.clone()),
            base_path: None,
        };
        match discover_torrent_candidates(&contents, &options) {
            Ok(candidates) => {
                push_discovery_candidates_for_provider(self.name(), &mut report, candidates)
            }
            Err(err) => report.diagnostic(
                TorrentSwarmProviderSeverity::Error,
                format!("failed to parse index feed {url}: {err}"),
                Some(url),
            ),
        }
        Ok(report)
    }
}

#[derive(Debug, Clone)]
pub struct DiscoverySwarmProvider {
    pub files: Vec<PathBuf>,
    pub urls: Vec<String>,
    pub input_kind: TorrentDiscoveryInputKind,
    pub timeout: Duration,
    client: reqwest::Client,
}

impl DiscoverySwarmProvider {
    pub fn new(
        files: Vec<PathBuf>,
        urls: Vec<String>,
        input_kind: TorrentDiscoveryInputKind,
        timeout: Duration,
    ) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(PROVIDER_USER_AGENT)
            .build()
            .map_err(|err| {
                Error::Other(format!(
                    "failed to build discovery provider HTTP client: {err}"
                ))
            })?;
        Ok(Self {
            files,
            urls,
            input_kind,
            timeout,
            client,
        })
    }

    fn push_discovery_candidates(
        &self,
        report: &mut TorrentSwarmProviderReport,
        candidates: Vec<TorrentDiscoveryCandidate>,
    ) {
        push_discovery_candidates_for_provider(self.name(), report, candidates);
    }
}

#[async_trait]
impl TorrentSwarmProvider for DiscoverySwarmProvider {
    fn name(&self) -> &str {
        "discovery"
    }

    async fn discover(
        &self,
        _context: TorrentSwarmProviderContext,
    ) -> Result<TorrentSwarmProviderReport, Error> {
        let mut report = TorrentSwarmProviderReport::new(self.name());
        for path in &self.files {
            match tokio::fs::read_to_string(path).await {
                Ok(contents) => {
                    let options = TorrentDiscoveryOptions {
                        input_kind: self.input_kind,
                        base_url: None,
                        base_path: discovery_file_base(path),
                    };
                    match discover_torrent_candidates(&contents, &options) {
                        Ok(candidates) => self.push_discovery_candidates(&mut report, candidates),
                        Err(err) => report.diagnostic(
                            TorrentSwarmProviderSeverity::Error,
                            format!("failed to parse discovery file {}: {err}", path.display()),
                            Some(path.display().to_string()),
                        ),
                    }
                }
                Err(err) => report.diagnostic(
                    TorrentSwarmProviderSeverity::Error,
                    format!("failed to read discovery file {}: {err}", path.display()),
                    Some(path.display().to_string()),
                ),
            }
        }

        for url in &self.urls {
            match self.client.get(url).send().await {
                Ok(response) => match response.error_for_status() {
                    Ok(response) => match response.text().await {
                        Ok(contents) => {
                            let options = TorrentDiscoveryOptions {
                                input_kind: self.input_kind,
                                base_url: Some(url.clone()),
                                base_path: None,
                            };
                            match discover_torrent_candidates(&contents, &options) {
                                Ok(candidates) => {
                                    self.push_discovery_candidates(&mut report, candidates)
                                }
                                Err(err) => report.diagnostic(
                                    TorrentSwarmProviderSeverity::Error,
                                    format!("failed to parse discovery URL {url}: {err}"),
                                    Some(url.clone()),
                                ),
                            }
                        }
                        Err(err) => report.diagnostic(
                            TorrentSwarmProviderSeverity::Error,
                            format!("failed to read discovery URL {url}: {err}"),
                            Some(url.clone()),
                        ),
                    },
                    Err(err) => report.diagnostic(
                        TorrentSwarmProviderSeverity::Error,
                        format!("failed to fetch discovery URL {url}: {err}"),
                        Some(url.clone()),
                    ),
                },
                Err(err) => report.diagnostic(
                    TorrentSwarmProviderSeverity::Error,
                    format!("failed to fetch discovery URL {url}: {err}"),
                    Some(url.clone()),
                ),
            }
        }
        Ok(report)
    }
}

pub fn build_swarm_provider_resolver(
    config: &SwarmProviderConfig,
    download_dir: &Path,
) -> Result<Option<TorrentSwarmProviderResolver>, Error> {
    if !config.enabled {
        return Ok(None);
    }

    let mut providers: Vec<Box<dyn TorrentSwarmProvider>> = Vec::new();
    if config.include_magnet_hints {
        providers.push(Box::new(MagnetHintProvider));
    }
    if !config.static_trackers.is_empty()
        || !config.static_tracker_files.is_empty()
        || !config.static_peers.is_empty()
        || !config.static_web_seeds.is_empty()
    {
        providers.push(Box::new(StaticSwarmProvider {
            trackers: config.static_trackers.clone(),
            tracker_files: config.static_tracker_files.clone(),
            peers: config.static_peers.clone(),
            web_seeds: config.static_web_seeds.clone(),
        }));
    }
    if !config.tracker_list_urls.is_empty() {
        providers.push(Box::new(TrackerListProvider::new(
            config.tracker_list_urls.clone(),
            Duration::from_secs(config.tracker_list_cache_ttl_secs),
            Duration::from_secs(config.tracker_list_timeout_secs),
        )?));
    }
    for index_provider in &config.index_providers {
        providers.push(Box::new(IndexFeedProvider::new(index_provider.clone())?));
    }
    if !config.discovery_files.is_empty() || !config.discovery_urls.is_empty() {
        providers.push(Box::new(DiscoverySwarmProvider::new(
            config.discovery_files.clone(),
            config.discovery_urls.clone(),
            config.discovery_input_kind,
            Duration::from_secs(config.discovery_timeout_secs),
        )?));
    }

    Ok(Some(TorrentSwarmProviderResolver::new(
        providers,
        config.limits,
        config.resolved_cache_dir(download_dir),
    )))
}

fn push_discovery_candidates_for_provider(
    provider_name: &str,
    report: &mut TorrentSwarmProviderReport,
    candidates: Vec<TorrentDiscoveryCandidate>,
) {
    for candidate in candidates {
        match candidate.kind {
            TorrentDiscoveryKind::Magnet => {
                report.push(TorrentSwarmProviderCandidate::locator(
                    TorrentSwarmProviderCandidateKind::Magnet,
                    candidate.locator,
                    candidate.display_name,
                    provider_name,
                    candidate.source,
                ));
            }
            TorrentDiscoveryKind::TorrentFile => {
                report.push(TorrentSwarmProviderCandidate::locator(
                    TorrentSwarmProviderCandidateKind::TorrentFile,
                    candidate.locator,
                    candidate.display_name,
                    provider_name,
                    candidate.source,
                ));
            }
            TorrentDiscoveryKind::Tracker => {
                report.push(TorrentSwarmProviderCandidate::tracker(
                    provider_name,
                    candidate.locator,
                    candidate.source,
                ));
            }
            TorrentDiscoveryKind::WebSeed => {
                report.push(TorrentSwarmProviderCandidate::web_seed(
                    provider_name,
                    candidate.locator,
                    candidate.source,
                ));
            }
        }
    }
}

fn expand_index_url_template(template: &str, spec: &DownloadSpec) -> Option<String> {
    let mut expanded = template.trim().to_string();
    if expanded.is_empty() {
        return None;
    }

    let info_hash = btih_from_spec(spec);
    let display_name = spec.file_name_hint();
    let locator = match spec {
        DownloadSpec::Metadata { .. } => None,
        _ => Some(spec.locator().to_string()),
    };
    let query = display_name
        .as_deref()
        .or(info_hash.as_deref())
        .or(locator.as_deref());

    for (token, value) in [
        ("btih", info_hash.as_deref()),
        ("info_hash", info_hash.as_deref()),
        ("display_name", display_name.as_deref()),
        ("query", query),
        ("locator", locator.as_deref()),
    ] {
        let placeholder = format!("{{{token}}}");
        if expanded.contains(&placeholder) {
            let value = value?;
            expanded = expanded.replace(&placeholder, &url_encode(value));
        }
    }

    Some(expanded)
}

fn btih_from_spec(spec: &DownloadSpec) -> Option<String> {
    match spec {
        DownloadSpec::Magnet { uri } => MagnetLink::parse(uri).ok().and_then(|magnet| {
            magnet
                .exact_topics
                .into_iter()
                .find_map(|topic| match topic {
                    super::MagnetExactTopic::Btih(hash) => Some(hash),
                    _ => None,
                })
        }),
        DownloadSpec::Metadata { info_hash, .. } => info_hash.clone(),
        _ => None,
    }
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn insert_candidate(
    resolution: &mut TorrentSwarmProviderResolution,
    seen: &mut HashSet<(TorrentSwarmProviderCandidateKind, String)>,
    candidate: TorrentSwarmProviderCandidate,
    limits: SwarmProviderLimits,
) -> bool {
    if !seen.insert((candidate.kind, candidate.locator.clone())) {
        return false;
    }
    match candidate.kind {
        TorrentSwarmProviderCandidateKind::Tracker => {
            if resolution.hints.trackers.len() >= limits.max_trackers {
                return false;
            }
            resolution.hints.add_tracker(candidate.locator.clone());
        }
        TorrentSwarmProviderCandidateKind::Peer => {
            if resolution.hints.peers.len() >= limits.max_peers {
                return false;
            }
            match TorrentPeerEndpoint::parse(&candidate.locator) {
                Ok(peer) => resolution.hints.add_peer(peer),
                Err(_) => return false,
            }
        }
        TorrentSwarmProviderCandidateKind::WebSeed => {
            if resolution.hints.web_seeds.len() >= limits.max_web_seeds {
                return false;
            }
            resolution.hints.add_web_seed(candidate.locator.clone());
        }
        TorrentSwarmProviderCandidateKind::Magnet
        | TorrentSwarmProviderCandidateKind::TorrentFile => {
            let locator_count = resolution
                .candidates
                .iter()
                .filter(|candidate| {
                    matches!(
                        candidate.kind,
                        TorrentSwarmProviderCandidateKind::Magnet
                            | TorrentSwarmProviderCandidateKind::TorrentFile
                    )
                })
                .count();
            if locator_count >= limits.max_locators {
                return false;
            }
        }
    }
    resolution.candidates.push(candidate);
    true
}

fn push_limited_diagnostic(
    resolution: &mut TorrentSwarmProviderResolution,
    diagnostic: TorrentSwarmProviderDiagnostic,
    limits: SwarmProviderLimits,
) {
    if resolution.diagnostics.len() < limits.max_diagnostics {
        resolution.diagnostics.push(diagnostic);
    }
}

fn seed_seen_from_hints(
    seen: &mut HashSet<(TorrentSwarmProviderCandidateKind, String)>,
    hints: &TorrentSwarmHints,
) {
    for tracker in &hints.trackers {
        seen.insert((TorrentSwarmProviderCandidateKind::Tracker, tracker.clone()));
    }
    for peer in &hints.peers {
        seen.insert((TorrentSwarmProviderCandidateKind::Peer, peer.to_string()));
    }
    for web_seed in &hints.web_seeds {
        seen.insert((TorrentSwarmProviderCandidateKind::WebSeed, web_seed.clone()));
    }
}

async fn is_fresh_cache(path: &Path, ttl: Duration) -> bool {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age <= ttl)
}

fn parse_tracker_lines(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect()
}

fn discovery_file_base(path: &Path) -> Option<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return canonical.parent().map(Path::to_path_buf);
    }
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

fn default_swarm_providers_enabled() -> bool {
    true
}

fn default_include_magnet_hints() -> bool {
    true
}

fn default_tracker_list_cache_ttl_secs() -> u64 {
    DEFAULT_TRACKER_LIST_CACHE_TTL_SECS
}

fn default_provider_timeout_secs() -> u64 {
    DEFAULT_PROVIDER_TIMEOUT_SECS
}

fn default_max_trackers() -> usize {
    128
}

fn default_max_peers() -> usize {
    64
}

fn default_max_web_seeds() -> usize {
    64
}

fn default_max_locators() -> usize {
    64
}

fn default_max_provider_diagnostics() -> usize {
    64
}

#[cfg(test)]
mod tests {
    use super::{
        DiscoverySwarmProvider, IndexFeedProvider, MagnetHintProvider, StaticSwarmProvider,
        SwarmIndexProviderConfig, TorrentSwarmProvider, TorrentSwarmProviderCandidateKind,
        TorrentSwarmProviderContext, TorrentSwarmProviderResolver, TrackerListProvider,
    };
    use crate::discovery::TorrentDiscoveryInputKind;
    use crate::domain::DownloadSpec;
    use crate::p2p::{SwarmProviderLimits, TorrentSwarmHints};
    use std::time::Duration;
    use tempfile::TempDir;

    #[tokio::test]
    async fn magnet_provider_extracts_standard_hints() {
        let provider = MagnetHintProvider;
        let report = provider
            .discover(TorrentSwarmProviderContext {
                spec: DownloadSpec::parse(
                    "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01&tr=udp%3A%2F%2Ftracker.example%2Fannounce&x.pe=127.0.0.1%3A6881&ws=https%3A%2F%2Fseed.example%2Fpayload",
                )
                .unwrap(),
                existing_hints: TorrentSwarmHints::default(),
                cache_dir: TempDir::new().unwrap().path().to_path_buf(),
            })
            .await
            .unwrap();

        assert!(report.candidates.iter().any(|candidate| {
            candidate.kind == TorrentSwarmProviderCandidateKind::Tracker
                && candidate.locator == "udp://tracker.example/announce"
        }));
        assert!(report.candidates.iter().any(|candidate| {
            candidate.kind == TorrentSwarmProviderCandidateKind::Peer
                && candidate.locator == "127.0.0.1:6881"
        }));
        assert!(report.candidates.iter().any(|candidate| {
            candidate.kind == TorrentSwarmProviderCandidateKind::WebSeed
                && candidate.locator == "https://seed.example/payload"
        }));
    }

    #[tokio::test]
    async fn resolver_merges_dedupes_and_limits_provider_candidates() {
        let temp = TempDir::new().unwrap();
        let resolver = TorrentSwarmProviderResolver::new(
            vec![Box::new(StaticSwarmProvider {
                trackers: vec![
                    "udp://tracker-one.example/announce".into(),
                    "udp://tracker-two.example/announce".into(),
                ],
                tracker_files: Vec::new(),
                peers: vec!["127.0.0.1:6881".into()],
                web_seeds: Vec::new(),
            })],
            SwarmProviderLimits {
                max_trackers: 1,
                max_peers: 8,
                max_web_seeds: 8,
                max_locators: 8,
                max_diagnostics: 8,
            },
            temp.path().to_path_buf(),
        );

        let resolution = resolver
            .resolve(
                DownloadSpec::parse("magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01")
                    .unwrap(),
                TorrentSwarmHints::default(),
            )
            .await;

        assert_eq!(
            resolution.hints.trackers,
            vec!["udp://tracker-one.example/announce"]
        );
        assert_eq!(resolution.hints.peers[0].to_string(), "127.0.0.1:6881");
    }

    #[tokio::test]
    async fn tracker_list_provider_uses_fresh_cache() {
        let temp = TempDir::new().unwrap();
        let provider = TrackerListProvider::new(
            vec!["https://example.com/trackers.txt".into()],
            Duration::from_secs(3600),
            Duration::from_secs(1),
        )
        .unwrap();
        let cache_path = provider.cache_path(temp.path(), "https://example.com/trackers.txt");
        tokio::fs::create_dir_all(cache_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &cache_path,
            "# comment\nudp://tracker-cache.example/announce\n\n",
        )
        .await
        .unwrap();

        let report = provider
            .discover(TorrentSwarmProviderContext {
                spec: DownloadSpec::parse(
                    "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01",
                )
                .unwrap(),
                existing_hints: TorrentSwarmHints::default(),
                cache_dir: temp.path().to_path_buf(),
            })
            .await
            .unwrap();

        assert!(report.candidates.iter().any(|candidate| {
            candidate.kind == TorrentSwarmProviderCandidateKind::Tracker
                && candidate.locator == "udp://tracker-cache.example/announce"
        }));
    }

    #[tokio::test]
    async fn index_provider_expands_btih_template_and_uses_fresh_cache() {
        let temp = TempDir::new().unwrap();
        let provider = IndexFeedProvider::new(SwarmIndexProviderConfig {
            name: "authorized".into(),
            url_template: "https://index.example/search?q={btih}".into(),
            input_kind: TorrentDiscoveryInputKind::Feed,
            cache_ttl_secs: 3600,
            timeout_secs: 1,
        })
        .unwrap();
        let url = "https://index.example/search?q=abcdef0123456789abcdef0123456789abcdef01";
        let cache_path = provider.cache_path(temp.path(), url);
        tokio::fs::create_dir_all(cache_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &cache_path,
            r#"<?xml version="1.0" encoding="utf-8"?>
            <feed xmlns="http://www.w3.org/2005/Atom">
              <title>Authorized</title>
              <entry>
                <title>Payload</title>
                <link href="magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01&amp;tr=udp%3A%2F%2Ftracker-index.example%2Fannounce" />
              </entry>
            </feed>"#,
        )
        .await
        .unwrap();

        let report = provider
            .discover(TorrentSwarmProviderContext {
                spec: DownloadSpec::parse(
                    "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01",
                )
                .unwrap(),
                existing_hints: TorrentSwarmHints::default(),
                cache_dir: temp.path().to_path_buf(),
            })
            .await
            .unwrap();

        assert_eq!(report.provider, "index-feed:authorized");
        assert!(report.candidates.iter().any(|candidate| {
            candidate.kind == TorrentSwarmProviderCandidateKind::Magnet
                && candidate.locator.starts_with("magnet:?xt=urn:btih:abcdef")
        }));
        assert!(report.candidates.iter().any(|candidate| {
            candidate.kind == TorrentSwarmProviderCandidateKind::Tracker
                && candidate.locator == "udp://tracker-index.example/announce"
        }));
    }

    #[tokio::test]
    async fn index_provider_reports_template_without_required_context() {
        let provider = IndexFeedProvider::new(SwarmIndexProviderConfig {
            name: "authorized".into(),
            url_template: "https://index.example/search?q={btih}".into(),
            input_kind: TorrentDiscoveryInputKind::Auto,
            cache_ttl_secs: 3600,
            timeout_secs: 1,
        })
        .unwrap();

        let report = provider
            .discover(TorrentSwarmProviderContext {
                spec: DownloadSpec::Metadata {
                    display_name: Some("linux iso".into()),
                    info_hash: None,
                },
                existing_hints: TorrentSwarmHints::default(),
                cache_dir: TempDir::new().unwrap().path().to_path_buf(),
            })
            .await
            .unwrap();

        assert!(report.candidates.is_empty());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("could not be expanded")
                && diagnostic.source.as_deref() == Some("https://index.example/search?q={btih}")
        }));
    }

    #[test]
    fn index_provider_rejects_invalid_config() {
        let err = IndexFeedProvider::new(SwarmIndexProviderConfig {
            name: " ".into(),
            url_template: "https://index.example/search?q={query}".into(),
            input_kind: TorrentDiscoveryInputKind::Auto,
            cache_ttl_secs: 3600,
            timeout_secs: 1,
        })
        .unwrap_err();

        assert!(err.to_string().contains("name cannot be blank"));
    }

    #[tokio::test]
    async fn discovery_provider_converts_html_candidates() {
        let temp = TempDir::new().unwrap();
        let page = temp.path().join("index.html");
        tokio::fs::write(
            &page,
            r#"<a href="payload.torrent">Payload</a><a href="udp://tracker.example/announce">Tracker</a>"#,
        )
        .await
        .unwrap();
        let provider = DiscoverySwarmProvider::new(
            vec![page],
            Vec::new(),
            TorrentDiscoveryInputKind::Html,
            Duration::from_secs(1),
        )
        .unwrap();

        let report = provider
            .discover(TorrentSwarmProviderContext {
                spec: DownloadSpec::parse(
                    "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01",
                )
                .unwrap(),
                existing_hints: TorrentSwarmHints::default(),
                cache_dir: temp.path().to_path_buf(),
            })
            .await
            .unwrap();

        assert!(report.candidates.iter().any(|candidate| {
            candidate.kind == TorrentSwarmProviderCandidateKind::TorrentFile
                && candidate.locator.ends_with("payload.torrent")
        }));
        assert!(report.candidates.iter().any(|candidate| {
            candidate.kind == TorrentSwarmProviderCandidateKind::Tracker
                && candidate.locator == "udp://tracker.example/announce"
        }));
    }
}
