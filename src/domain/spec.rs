use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadSpec {
    Http { url: String },
    Https { url: String },
    Ftp { url: String },
}

impl DownloadSpec {
    pub fn parse(locator: impl Into<String>) -> Result<Self, Error> {
        let locator = locator.into();
        let parsed = url::Url::parse(&locator)?;

        match parsed.scheme() {
            "http" => Ok(Self::Http { url: locator }),
            "https" => Ok(Self::Https { url: locator }),
            "ftp" => Ok(Self::Ftp { url: locator }),
            other => Err(Error::UnsupportedProtocol(other.to_string())),
        }
    }

    pub fn locator(&self) -> &str {
        match self {
            Self::Http { url } | Self::Https { url } | Self::Ftp { url } => url,
        }
    }

    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Http { .. } => "http",
            Self::Https { .. } => "https",
            Self::Ftp { .. } => "ftp",
        }
    }

    pub fn file_name_hint(&self) -> Option<String> {
        url::Url::parse(self.locator())
            .ok()
            .and_then(|url| {
                url.path_segments()
                    .and_then(|segments| segments.last())
                    .filter(|segment| !segment.is_empty())
                    .map(|segment| segment.to_string())
            })
    }

    pub fn supports_origin_discovery(&self) -> bool {
        matches!(self, Self::Http { .. } | Self::Https { .. })
    }
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
    use crate::error::Error;

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
        let err = DownloadSpec::parse("magnet:?xt=urn:btih:deadbeef").unwrap_err();
        assert!(matches!(err, Error::UnsupportedProtocol(_)));
    }
}
