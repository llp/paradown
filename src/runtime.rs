use crate::config::Config;
use crate::domain::{HttpAuth, HttpRequestOptions, ProxyOptions, TlsOptions};
use crate::error::Error;
use log::LevelFilter;
use reqwest::header::{COOKIE, HeaderName, HeaderValue, USER_AGENT};
use reqwest::{Certificate, Identity, NoProxy, Proxy, RequestBuilder};
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
    builder = apply_cookie_store(builder, config.http.client.cookie_store);
    builder = apply_tls_options(builder, &config.http.client.tls)?;

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
            Error::Other(format!("Invalid value for header '{}': {err}", header.name))
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

fn apply_cookie_store(
    builder: reqwest::ClientBuilder,
    cookie_store_enabled: bool,
) -> reqwest::ClientBuilder {
    if cookie_store_enabled {
        builder.cookie_store(true)
    } else {
        builder
    }
}

fn apply_tls_options(
    mut builder: reqwest::ClientBuilder,
    tls: &TlsOptions,
) -> Result<reqwest::ClientBuilder, Error> {
    if tls.insecure_skip_verify {
        builder = builder.danger_accept_invalid_certs(true);
    }

    if let Some(path) = &tls.ca_certificate_pem {
        let pem = std::fs::read(path).map_err(|err| {
            Error::Other(format!(
                "Failed to read CA certificate PEM '{}': {err}",
                path.display()
            ))
        })?;
        let certificate = Certificate::from_pem(&pem).map_err(|err| {
            Error::Other(format!(
                "Invalid CA certificate PEM '{}': {err}",
                path.display()
            ))
        })?;
        builder = builder.add_root_certificate(certificate);
    }

    if let Some(path) = &tls.client_identity_pem {
        builder = builder.use_rustls_tls();
        let pem = std::fs::read(path).map_err(|err| {
            Error::Other(format!(
                "Failed to read client identity PEM '{}': {err}",
                path.display()
            ))
        })?;
        let identity = Identity::from_pem(&pem).map_err(|err| {
            Error::Other(format!(
                "Invalid client identity PEM '{}': {err}",
                path.display()
            ))
        })?;
        builder = builder.identity(identity);
    }

    Ok(builder)
}
