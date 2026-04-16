pub use crate::coordinator::Manager;
pub use crate::domain::{
    BlockState, DownloadSpec, FileManifest, HttpResourceIdentity, PieceBlock, PieceLayout,
    PieceState, SessionDescriptor, SessionManifest, SessionMode, SourceCapabilities,
    SourceDescriptor, SourceKind, SourceSet,
};
pub use crate::events::Event;
pub use crate::job::{Task, TaskSnapshot};
pub use crate::request::{SegmentRequest, SegmentRequestBuilder, TaskRequest, TaskRequestBuilder};
pub use crate::stats::StatsSnapshot;
pub use crate::status::Status;
pub use crate::worker::Worker;

pub type Session = Task;
pub type SessionSnapshot = TaskSnapshot;
pub type SessionRequest = TaskRequest;
