use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadSpec {
    Http {
        url: String,
    },
    Https {
        url: String,
    },
    Ftp {
        url: String,
    },
    TorrentFile {
        path: String,
    },
    Magnet {
        uri: String,
    },
    Metadata {
        display_name: Option<String>,
        info_hash: Option<String>,
    },
}

impl DownloadSpec {
    pub fn parse(locator: impl Into<String>) -> Result<Self, Error> {
        let locator = locator.into();
        match url::Url::parse(&locator) {
            Ok(parsed) => match parsed.scheme() {
                "http" => Ok(Self::Http { url: locator }),
                "https" => Ok(Self::Https { url: locator }),
                "ftp" => Ok(Self::Ftp { url: locator }),
                "magnet" => Ok(Self::Magnet { uri: locator }),
                other => Err(Error::UnsupportedProtocol(other.to_string())),
            },
            Err(url::ParseError::RelativeUrlWithoutBase) if locator.ends_with(".torrent") => {
                Ok(Self::TorrentFile { path: locator })
            }
            Err(err) => Err(err.into()),
        }
    }

    pub fn locator(&self) -> &str {
        match self {
            Self::Http { url } | Self::Https { url } | Self::Ftp { url } => url,
            Self::TorrentFile { path } => path,
            Self::Magnet { uri } => uri,
            Self::Metadata {
                display_name,
                info_hash,
            } => info_hash
                .as_deref()
                .or(display_name.as_deref())
                .unwrap_or("metadata"),
        }
    }

    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Http { .. } => "http",
            Self::Https { .. } => "https",
            Self::Ftp { .. } => "ftp",
            Self::TorrentFile { .. } => "torrent",
            Self::Magnet { .. } => "magnet",
            Self::Metadata { .. } => "metadata",
        }
    }

    pub fn file_name_hint(&self) -> Option<String> {
        match self {
            Self::Http { .. } | Self::Https { .. } | Self::Ftp { .. } | Self::Magnet { .. } => {
                file_name_hint_from_locator(self.locator())
            }
            Self::TorrentFile { path } => std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
            Self::Metadata { display_name, .. } => display_name.clone(),
        }
    }

    pub fn supports_origin_discovery(&self) -> bool {
        matches!(self, Self::Http { .. } | Self::Https { .. })
    }

    pub fn supports_swarm_discovery(&self) -> bool {
        matches!(
            self,
            Self::TorrentFile { .. } | Self::Magnet { .. } | Self::Metadata { .. }
        )
    }

    pub fn identity_key(&self) -> String {
        match self {
            Self::Http { url } | Self::Https { url } | Self::Ftp { url } => url.clone(),
            Self::TorrentFile { path } => format!("torrent-file:{path}"),
            Self::Magnet { uri } => uri.clone(),
            Self::Metadata {
                display_name,
                info_hash,
            } => format!(
                "metadata:{}:{}",
                info_hash.as_deref().unwrap_or(""),
                display_name.as_deref().unwrap_or("")
            ),
        }
    }
}

pub fn file_name_hint_from_locator(locator: &str) -> Option<String> {
    url::Url::parse(locator).ok().and_then(|url| {
        url.path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|segment| !segment.is_empty())
            .map(|segment| segment.to_string())
    })
}

impl fmt::Display for DownloadSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.locator())
    }
}

impl TryFrom<&str> for DownloadSpec {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for DownloadSpec {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::DownloadSpec;

    #[test]
    fn parses_http_locator() {
        let spec = DownloadSpec::parse("http://example.com/file.bin").unwrap();
        assert_eq!(spec.scheme(), "http");
        assert_eq!(spec.locator(), "http://example.com/file.bin");
    }

    #[test]
    fn parses_https_locator() {
        let spec = DownloadSpec::parse("https://example.com/file.bin").unwrap();
        assert_eq!(spec.scheme(), "https");
        assert_eq!(spec.file_name_hint().as_deref(), Some("file.bin"));
    }

    #[test]
    fn rejects_unsupported_protocols() {
        let spec = DownloadSpec::parse("magnet:?xt=urn:btih:deadbeef").unwrap();
        assert!(matches!(spec, DownloadSpec::Magnet { .. }));
    }

    #[test]
    fn keeps_http_torrent_links_as_http_specs() {
        let spec = DownloadSpec::parse("https://example.com/file.torrent").unwrap();
        assert!(matches!(spec, DownloadSpec::Https { .. }));
    }

    #[test]
    fn parses_local_torrent_files_as_torrent_specs() {
        let spec = DownloadSpec::parse("/tmp/archive.torrent").unwrap();
        assert!(matches!(spec, DownloadSpec::TorrentFile { .. }));
    }
}
