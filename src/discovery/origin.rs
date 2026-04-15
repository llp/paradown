use crate::domain::{DownloadSpec, HttpRequestOptions, HttpResourceIdentity};
use crate::error::Error;
use crate::protocol_probe::probe_download_target;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OriginMetadata {
    pub total_size: u64,
    pub supports_range_requests: bool,
    pub resource_identity: HttpResourceIdentity,
    pub suggested_file_name: Option<String>,
}

#[async_trait]
pub(crate) trait DiscoveryDriver: Send + Sync {
    async fn discover(
        &self,
        client: &reqwest::Client,
        spec: &DownloadSpec,
        request: &HttpRequestOptions,
    ) -> Result<OriginMetadata, Error>;
}

struct HttpDiscoveryDriver;
struct FtpDiscoveryDriver;

static HTTP_DISCOVERY_DRIVER: HttpDiscoveryDriver = HttpDiscoveryDriver;
static FTP_DISCOVERY_DRIVER: FtpDiscoveryDriver = FtpDiscoveryDriver;

pub(crate) async fn discover_origin(
    client: &reqwest::Client,
    spec: &DownloadSpec,
    request: &HttpRequestOptions,
) -> Result<OriginMetadata, Error> {
    let driver: &dyn DiscoveryDriver = match spec {
        DownloadSpec::Http { .. } | DownloadSpec::Https { .. } => &HTTP_DISCOVERY_DRIVER,
        DownloadSpec::Ftp { .. } => &FTP_DISCOVERY_DRIVER,
    };

    driver.discover(client, spec, request).await
}

#[async_trait]
impl DiscoveryDriver for HttpDiscoveryDriver {
    async fn discover(
        &self,
        client: &reqwest::Client,
        spec: &DownloadSpec,
        request: &HttpRequestOptions,
    ) -> Result<OriginMetadata, Error> {
        let probe = probe_download_target(client, spec.locator(), request).await?;
        Ok(OriginMetadata {
            total_size: probe.total_size,
            supports_range_requests: probe.supports_range_requests,
            resource_identity: probe.resource_identity,
            suggested_file_name: probe.suggested_file_name,
        })
    }
}

#[async_trait]
impl DiscoveryDriver for FtpDiscoveryDriver {
    async fn discover(
        &self,
        _client: &reqwest::Client,
        _spec: &DownloadSpec,
        _request: &HttpRequestOptions,
    ) -> Result<OriginMetadata, Error> {
        Err(Error::UnsupportedProtocol(
            "ftp discovery is not implemented yet".into(),
        ))
    }
}
