use crate::Error;

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
    Cancel(u32),
    Delete(u32),
}
