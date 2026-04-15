use crate::error::Error;
use crate::transfer::driver::TransferDriver;
use crate::worker::Worker;
use crate::worker::transfer::ProgressReporter;
use async_trait::async_trait;

pub(crate) struct FtpTransferDriver;

#[async_trait]
impl TransferDriver for FtpTransferDriver {
    fn build_request(
        &self,
        _worker: &Worker,
        _range_start: u64,
        _use_range_requests: bool,
    ) -> Result<reqwest::RequestBuilder, Error> {
        Err(Error::UnsupportedProtocol(
            "ftp transfer is not implemented yet".into(),
        ))
    }

    fn resolve_content_length(
        &self,
        _worker: &Worker,
        _response: &reqwest::Response,
        _use_range_requests: bool,
        _range_start: u64,
    ) -> u64 {
        0
    }

    fn validate_response(
        &self,
        _worker: &Worker,
        _response: &reqwest::Response,
        _use_range_requests: bool,
        _expected_start: u64,
    ) -> Result<(), Error> {
        Err(Error::UnsupportedProtocol(
            "ftp transfer is not implemented yet".into(),
        ))
    }

    async fn stream_response(
        &self,
        _worker: &Worker,
        _response: reqwest::Response,
        _downloaded_size: &mut u64,
        _reporter: &mut ProgressReporter,
        _use_range_requests: bool,
        _range_start: u64,
    ) -> Result<(), Error> {
        Err(Error::UnsupportedProtocol(
            "ftp transfer is not implemented yet".into(),
        ))
    }
}
