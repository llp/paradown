use crate::domain::{DownloadSpec, HttpRequestOptions, HttpResourceIdentity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Http,
    Https,
    Ftp,
    TorrentFile,
    Magnet,
    Metadata,
    Tracker,
    Dht,
    Peer,
    WebSeed,
    Mirror,
    OfflineCache,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCapabilities {
    pub metadata_discovery: bool,
    pub random_access: bool,
    pub range_requests: bool,
    pub uploads: bool,
    pub dynamic_availability: bool,
}

impl SourceCapabilities {
    pub fn http_origin() -> Self {
        Self {
            metadata_discovery: false,
            random_access: true,
            range_requests: true,
            uploads: false,
            dynamic_availability: false,
        }
    }

    pub fn ftp_origin() -> Self {
        Self {
            metadata_discovery: false,
            random_access: true,
            range_requests: false,
            uploads: false,
            dynamic_availability: false,
        }
    }

    pub fn metadata_only() -> Self {
        Self {
            metadata_discovery: true,
            random_access: false,
            range_requests: false,
            uploads: false,
            dynamic_availability: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDescriptor {
    pub id: String,
    pub kind: SourceKind,
    pub locator: String,
    pub metadata_only: bool,
    pub request: Option<HttpRequestOptions>,
    pub resource_identity: Option<HttpResourceIdentity>,
    pub capabilities: SourceCapabilities,
}

impl SourceDescriptor {
    pub fn from_spec(spec: &DownloadSpec, request: Option<HttpRequestOptions>) -> Self {
        match spec {
            DownloadSpec::Http { url } => Self {
                id: format!("http::{url}"),
                kind: SourceKind::Http,
                locator: url.clone(),
                metadata_only: false,
                request,
                resource_identity: None,
                capabilities: SourceCapabilities::http_origin(),
            },
            DownloadSpec::Https { url } => Self {
                id: format!("https::{url}"),
                kind: SourceKind::Https,
                locator: url.clone(),
                metadata_only: false,
                request,
                resource_identity: None,
                capabilities: SourceCapabilities::http_origin(),
            },
            DownloadSpec::Ftp { url } => Self {
                id: format!("ftp::{url}"),
                kind: SourceKind::Ftp,
                locator: url.clone(),
                metadata_only: false,
                request,
                resource_identity: None,
                capabilities: SourceCapabilities::ftp_origin(),
            },
            DownloadSpec::TorrentFile { path } => Self {
                id: format!("torrent::{path}"),
                kind: SourceKind::TorrentFile,
                locator: path.clone(),
                metadata_only: true,
                request: None,
                resource_identity: None,
                capabilities: SourceCapabilities::metadata_only(),
            },
            DownloadSpec::Magnet { uri } => Self {
                id: format!("magnet::{uri}"),
                kind: SourceKind::Magnet,
                locator: uri.clone(),
                metadata_only: true,
                request: None,
                resource_identity: None,
                capabilities: SourceCapabilities::metadata_only(),
            },
            DownloadSpec::Metadata {
                display_name,
                info_hash,
            } => {
                let stable_key = info_hash
                    .clone()
                    .or_else(|| display_name.clone())
                    .unwrap_or_else(|| "metadata".into());
                Self {
                    id: format!("metadata::{stable_key}"),
                    kind: SourceKind::Metadata,
                    locator: stable_key,
                    metadata_only: true,
                    request: None,
                    resource_identity: None,
                    capabilities: SourceCapabilities::metadata_only(),
                }
            }
        }
    }

    pub fn with_identity(mut self, resource_identity: HttpResourceIdentity) -> Self {
        self.resource_identity = Some(resource_identity);
        self
    }

    pub fn supports_range_requests(&self) -> bool {
        self.capabilities.range_requests
    }

    pub fn can_transfer_payload(&self) -> bool {
        !self.metadata_only
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSet {
    pub primary_id: Option<String>,
    pub sources: Vec<SourceDescriptor>,
}

impl SourceSet {
    pub fn single_primary(source: SourceDescriptor) -> Self {
        Self {
            primary_id: Some(source.id.clone()),
            sources: vec![source],
        }
    }

    pub fn for_spec(spec: &DownloadSpec, request: Option<HttpRequestOptions>) -> Self {
        Self::single_primary(SourceDescriptor::from_spec(spec, request))
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn primary(&self) -> Option<&SourceDescriptor> {
        self.primary_id
            .as_ref()
            .and_then(|id| self.sources.iter().find(|source| &source.id == id))
            .or_else(|| self.sources.first())
    }

    pub fn primary_mut(&mut self) -> Option<&mut SourceDescriptor> {
        if let Some(primary_id) = self.primary_id.as_ref() {
            return self
                .sources
                .iter_mut()
                .find(|source| &source.id == primary_id);
        }

        self.sources.first_mut()
    }

    pub fn get(&self, source_id: &str) -> Option<&SourceDescriptor> {
        self.sources.iter().find(|source| source.id == source_id)
    }

    pub fn active_transfer_sources(&self) -> Vec<&SourceDescriptor> {
        self.sources
            .iter()
            .filter(|source| source.can_transfer_payload())
            .collect()
    }

    pub fn replace_primary(&mut self, source: SourceDescriptor) {
        self.primary_id = Some(source.id.clone());
        if let Some(existing) = self
            .sources
            .iter_mut()
            .find(|existing| existing.id == source.id)
        {
            *existing = source;
            return;
        }

        self.sources.insert(0, source);
    }
}

#[cfg(test)]
mod tests {
    use super::{SourceDescriptor, SourceKind, SourceSet};
    use crate::domain::DownloadSpec;

    #[test]
    fn builds_single_http_source_set_from_spec() {
        let spec = DownloadSpec::parse("https://example.com/file.bin").unwrap();
        let sources = SourceSet::for_spec(&spec, None);

        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources.primary().map(|source| source.kind.clone()),
            Some(SourceKind::Https)
        );
        assert_eq!(
            sources
                .primary()
                .map(|source| source.locator.clone())
                .as_deref(),
            Some("https://example.com/file.bin")
        );
    }

    #[test]
    fn replaces_existing_primary_source_by_id() {
        let mut sources = SourceSet::single_primary(SourceDescriptor::from_spec(
            &DownloadSpec::parse("https://example.com/a.bin").unwrap(),
            None,
        ));
        let replacement = SourceDescriptor {
            id: "https::https://example.com/a.bin".into(),
            kind: SourceKind::Https,
            locator: "https://cdn.example.com/a.bin".into(),
            metadata_only: false,
            request: None,
            resource_identity: None,
            capabilities: super::SourceCapabilities::http_origin(),
        };

        sources.replace_primary(replacement);

        assert_eq!(
            sources
                .primary()
                .map(|source| source.locator.clone())
                .as_deref(),
            Some("https://cdn.example.com/a.bin")
        );
    }
}
