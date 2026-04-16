use crate::domain::{SourceDescriptor, SourceKind};
use crate::error::Error;
use crate::transfer::ftp::FtpTransferDriver;
use crate::transfer::http::HttpTransferDriver;
use crate::worker::Worker;
use crate::worker::transfer::ProgressReporter;
use async_trait::async_trait;

#[async_trait]
pub(crate) trait TransferDriver: Send + Sync {
    async fn build_request(
        &self,
        worker: &Worker,
        range_start: u64,
        use_range_requests: bool,
    ) -> Result<reqwest::RequestBuilder, Error>;

    fn resolve_content_length(
        &self,
        worker: &Worker,
        response: &reqwest::Response,
        use_range_requests: bool,
        range_start: u64,
    ) -> u64;

    fn validate_response(
        &self,
        worker: &Worker,
        response: &reqwest::Response,
        use_range_requests: bool,
        expected_start: u64,
    ) -> Result<(), Error>;

    async fn stream_response(
        &self,
        worker: &Worker,
        response: reqwest::Response,
        downloaded_size: &mut u64,
        reporter: &mut ProgressReporter,
        use_range_requests: bool,
        range_start: u64,
    ) -> Result<(), Error>;
}

static HTTP_TRANSFER_DRIVER: HttpTransferDriver = HttpTransferDriver;
static FTP_TRANSFER_DRIVER: FtpTransferDriver = FtpTransferDriver;

pub(crate) fn driver_for_source(source: &SourceDescriptor) -> &'static dyn TransferDriver {
    match source.kind {
        SourceKind::Http | SourceKind::Https | SourceKind::WebSeed | SourceKind::Mirror => {
            &HTTP_TRANSFER_DRIVER
        }
        SourceKind::Ftp => &FTP_TRANSFER_DRIVER,
        _ => &FTP_TRANSFER_DRIVER,
    }
}
