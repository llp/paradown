//! `domain` 是领域类型模块的门面。
//!
//! 这个文件本身通常不放大量实现，而是：
//! - 用 `mod` 声明同目录下的子模块。
//! - 用 `pub use` 把常用类型重新导出给父模块。
//!
//! 因为 `src/lib.rs` 里写的是私有 `mod domain;`，外部用户不能直接访问
//! `paradown::domain::...`。但是 `lib.rs` 会继续 `pub use domain::{...}`，
//! 所以这些类型最终仍可作为 `paradown::SourceDescriptor` 等公共 API 使用。

// 这些子模块默认是私有的。父模块 `domain` 可以访问它们，兄弟模块需要通过 re-export 使用公开项。
mod http;
mod manifest;
mod piece;
mod session;
mod source;
mod spec;

// `pub use http::{...};` 把 `http` 子模块中的公开类型提升到 `domain` 模块这一层。
// 这是一种“模块门面”写法：隐藏文件拆分细节，暴露更稳定的类型集合。
pub use http::{
    HttpAuth, HttpClientOptions, HttpConfig, HttpHeader, HttpRequestOptions, HttpResourceIdentity,
    ProxyOptions, TlsOptions,
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
