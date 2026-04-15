mod http;
mod manifest;
mod piece;
mod spec;

pub use http::{
    HttpAuth, HttpClientOptions, HttpConfig, HttpHeader, HttpRequestOptions,
    HttpResourceIdentity, ProxyOptions,
};
pub use manifest::{FileManifest, SessionManifest};
pub use piece::{
    PieceBlock, PieceLayout, PieceState, completed_piece_count, initialize_piece_states,
    mark_completed_pieces, plan_piece_layouts,
};
pub use spec::{DownloadSpec, file_name_hint_from_locator};
