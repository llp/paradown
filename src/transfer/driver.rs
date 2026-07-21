use crate::domain::{SourceDescriptor, SourceKind};
use crate::error::Error;
use crate::transfer::ftp::FtpTransferDriver;
use crate::transfer::http::HttpTransferDriver;
use crate::worker::Worker;
use crate::worker::transfer::ProgressReporter;
use async_trait::async_trait;

/// 传输驱动 trait。
///
/// `trait` 定义“某类类型必须具备哪些方法”。HTTP 驱动和 FTP 驱动可以是不同具体类型，
/// 但只要都实现 `TransferDriver`，调用者就能通过同一套方法使用它们。
///
/// `: Send + Sync` 是 trait bound，表示实现者必须能安全地跨线程移动和共享引用。
/// 这对异步下载器很重要，因为驱动可能被多个任务引用。
///
/// `#[async_trait]` 是过程宏。它让 trait 中可以写 `async fn`，并在编译时生成必要的包装代码。
/// 泛型和 trait object 的系统解释见 `docs/rust/generics-and-traits.md`。
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

/// 根据来源选择传输驱动。
///
/// 返回类型 `&'static dyn TransferDriver` 同时包含两个语法点：
/// - `&'static`：返回的是对静态全局驱动实例的引用，程序整个运行期间有效。
/// - `dyn TransferDriver`：trait object，表示具体驱动类型可以是 HTTP，也可以是 FTP。
///
/// 这里用动态分发的好处是调用者不需要关心返回的是哪种具体驱动，只需要调用 trait 方法。
pub(crate) fn driver_for_source(source: &SourceDescriptor) -> &'static dyn TransferDriver {
    match source.kind {
        SourceKind::Http | SourceKind::Https | SourceKind::WebSeed | SourceKind::Mirror => {
            &HTTP_TRANSFER_DRIVER
        }
        SourceKind::Ftp => &FTP_TRANSFER_DRIVER,
        _ => &FTP_TRANSFER_DRIVER,
    }
}
