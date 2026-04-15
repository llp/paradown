use crate::config::DownloadConfig;
use crate::error::DownloadError;
use log::LevelFilter;
use std::time::Duration;

pub fn init_logger(debug: bool) {
    let log_level = if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    let mut builder = env_logger::Builder::from_default_env();
    builder
        .filter_level(log_level)
        .filter_module("sqlx::query", LevelFilter::Info);

    let _ = builder.try_init();
}

pub(crate) fn build_http_client(config: &DownloadConfig) -> Result<reqwest::Client, DownloadError> {
    reqwest::Client::builder()
        .connect_timeout(config.connection_timeout)
        .timeout(Duration::from_secs(300))
        .pool_max_idle_per_host(50)
        .pool_idle_timeout(Duration::from_secs(60))
        .gzip(true)
        .build()
        .map_err(|e| DownloadError::Other(format!("Failed to build HTTP client: {}", e)))
}
