mod http;
mod manifest;
mod piece;
mod session;
mod source;
mod spec;

pub use http::{
    HttpAuth, HttpClientOptions, HttpConfig, HttpHeader, HttpRequestOptions, HttpResourceIdentity,
    ProxyOptions,
};
pub use manifest::{FileManifest, SessionManifest};
pub use piece::{
    BlockState, PieceBlock, PieceLayout, PieceState, completed_block_count, completed_piece_count,
    derive_piece_states_from_blocks, initialize_block_states, initialize_piece_states,
    mark_completed_blocks, plan_piece_blocks, plan_piece_layouts,
};
pub use session::{SessionDescriptor, SessionMode};
pub use source::{SourceCapabilities, SourceDescriptor, SourceKind, SourceSet};
pub use spec::{DownloadSpec, file_name_hint_from_locator};
