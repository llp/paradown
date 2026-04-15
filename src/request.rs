mod job_request;
mod segment_request;

pub use job_request::{
    DownloadJobRequest, DownloadJobRequestBuilder, DownloadRequestBuilder, DownloadTaskRequest,
};
pub use segment_request::{
    DownloadWorkerBuilder, DownloadWorkerRequest, SegmentRequest, SegmentRequestBuilder,
};
