use crate::Error;
use crate::p2p::TorrentDiagnosticEvent;

#[derive(Debug, Clone)]
pub enum Event {
    Pending(u32),
    Preparing(u32),
    Start(u32),
    Pause(u32),
    Progress {
        id: u32,
        downloaded: u64,
        total: u64,
    },
    Complete(u32),
    Error(u32, Error),
    TorrentDiagnostic {
        id: u32,
        diagnostic: TorrentDiagnosticEvent,
    },
    Cancel(u32),
    Delete(u32),
}
