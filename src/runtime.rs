use crate::config::Config;
use crate::domain::{HttpAuth, HttpRequestOptions, ProxyOptions};
use crate::error::Error;
use log::LevelFilter;
use reqwest::header::{COOKIE, HeaderName, HeaderValue, USER_AGENT};
use reqwest::{NoProxy, Proxy, RequestBuilder};
use std::time::Duration;

pub fn init_logger(debug: bool) {
    let log_level = if debug {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    init_logger_with_level(log_level);
}

pub fn init_logger_with_level(log_level: LevelFilter) {
    let mut builder = env_logger::Builder::from_default_env();
    builder
        .filter_level(log_level)
        .filter_module("sqlx::query", LevelFilter::Info);

    let _ = builder.try_init();
}

pub(crate) fn build_http_client(config: &Config) -> Result<reqwest::Client, Error> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(config.connection_timeout)
        .timeout(Duration::from_secs(300))
        .pool_max_idle_per_host(50)
        .pool_idle_timeout(Duration::from_secs(60))
        .gzip(true);

    builder = apply_proxy_options(builder, &config.http.client.proxy)?;

    builder
        .build()
        .map_err(|e| Error::Other(format!("Failed to build HTTP client: {}", e)))
}

pub(crate) fn apply_http_request_options(
    mut builder: RequestBuilder,
    options: &HttpRequestOptions,
) -> Result<RequestBuilder, Error> {
    for header in &options.headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|err| Error::Other(format!("Invalid header name '{}': {err}", header.name)))?;
        let value = HeaderValue::from_str(&header.value).map_err(|err| {
            Error::Other(format!(
                "Invalid value for header '{}': {err}",
                header.name
            ))
        })?;
        builder = builder.header(name, value);
    }

    if let Some(cookie) = &options.cookie {
        builder = builder.header(COOKIE, cookie);
    }
    if let Some(user_agent) = &options.user_agent {
        builder = builder.header(USER_AGENT, user_agent);
    }

    builder = match &options.auth {
        Some(HttpAuth::Basic { username, password }) => {
            builder.basic_auth(username.clone(), password.clone())
        }
        Some(HttpAuth::Bearer { token }) => builder.bearer_auth(token),
        None => builder,
    };

    Ok(builder)
}

fn apply_proxy_options(
    mut builder: reqwest::ClientBuilder,
    proxy: &ProxyOptions,
) -> Result<reqwest::ClientBuilder, Error> {
    if !proxy.use_env_proxy {
        builder = builder.no_proxy();
    }

    if let Some(proxy_url) = &proxy.http_proxy {
        let mut http_proxy = Proxy::http(proxy_url)
            .map_err(|err| Error::Other(format!("Invalid HTTP proxy '{}': {err}", proxy_url)))?;
        if let Some(no_proxy) = &proxy.no_proxy {
            http_proxy = http_proxy.no_proxy(NoProxy::from_string(no_proxy));
        }
        builder = builder.proxy(http_proxy);
    }

    if let Some(proxy_url) = &proxy.https_proxy {
        let mut https_proxy = Proxy::https(proxy_url)
            .map_err(|err| Error::Other(format!("Invalid HTTPS proxy '{}': {err}", proxy_url)))?;
        if let Some(no_proxy) = &proxy.no_proxy {
            https_proxy = https_proxy.no_proxy(NoProxy::from_string(no_proxy));
        }
        builder = builder.proxy(https_proxy);
    }

    Ok(builder)
}
