//! 请求类型模块门面。
//!
//! `segment` 和 `task` 是当前模块的私有子模块；本文件通过 `pub use`
//! 只导出调用者真正需要的请求类型和 builder 类型。
//! 这种写法能让外部 API 保持简洁，同时允许内部继续拆文件组织代码。

mod segment;
mod task;

// `pub use` 重新导出后，父模块可以通过 `request::SegmentRequest` 使用类型，
// 而不必写成 `request::segment::SegmentRequest`。
pub use segment::{SegmentRequest, SegmentRequestBuilder};
pub use task::{TaskRequest, TaskRequestBuilder};
