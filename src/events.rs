use crate::DownloadError;

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Preparing(u32),
    Start(u32),
    Pause(u32),
    Progress {
        id: u32,
        downloaded: u64,
        total: u64,
    },
    Complete(u32),
    Error(u32, DownloadError),
    Cancel(u32),
}