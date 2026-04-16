use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResourceIdentity {
    pub resolved_url: Option<String>,
    pub entity_tag: Option<String>,
    pub last_modified: Option<String>,
}

impl HttpResourceIdentity {
    pub fn resume_validator(&self) -> Option<&str> {
        self.entity_tag.as_deref().or(self.last_modified.as_deref())
    }

    pub fn has_resume_validator(&self) -> bool {
        self.resume_validator().is_some()
    }

    pub fn validator_changed(&self, fresh: &Self) -> bool {
        match (self.entity_tag.as_deref(), fresh.entity_tag.as_deref()) {
            (Some(current), Some(next)) => current != next,
            (Some(_), None) => true,
            _ => match (
                self.last_modified.as_deref(),
                fresh.last_modified.as_deref(),
            ) {
                (Some(current), Some(next)) => current != next,
                (Some(_), None) => true,
                _ => false,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpAuth {
    Basic {
        username: String,
        password: Option<String>,
    },
    Bearer {
        token: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequestOptions {
    #[serde(default)]
    pub headers: Vec<HttpHeader>,
    pub cookie: Option<String>,
    pub auth: Option<HttpAuth>,
    pub user_agent: Option<String>,
}

impl HttpRequestOptions {
    pub fn merged(&self, overrides: &Self) -> Self {
        let mut headers = self.headers.clone();
        headers.extend(overrides.headers.iter().cloned());

        Self {
            headers,
            cookie: overrides.cookie.clone().or_else(|| self.cookie.clone()),
            auth: overrides.auth.clone().or_else(|| self.auth.clone()),
            user_agent: overrides
                .user_agent
                .clone()
                .or_else(|| self.user_agent.clone()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
            && self.cookie.is_none()
            && self.auth.is_none()
            && self.user_agent.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyOptions {
    #[serde(default = "default_true")]
    pub use_env_proxy: bool,
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub no_proxy: Option<String>,
}

impl Default for ProxyOptions {
    fn default() -> Self {
        Self {
            use_env_proxy: true,
            http_proxy: None,
            https_proxy: None,
            no_proxy: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsOptions {
    #[serde(default)]
    pub insecure_skip_verify: bool,
    pub ca_certificate_pem: Option<PathBuf>,
    pub client_identity_pem: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpClientOptions {
    #[serde(default)]
    pub proxy: ProxyOptions,
    #[serde(default)]
    pub cookie_store: bool,
    #[serde(default)]
    pub tls: TlsOptions,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpConfig {
    #[serde(default)]
    pub client: HttpClientOptions,
    #[serde(default)]
    pub request: HttpRequestOptions,
}

fn default_true() -> bool {
    true
}
