use chrono::{DateTime, Utc};

/// 分片请求。
///
/// 这个结构体适合作为学习 `Option<T>` 字段的例子：
/// 有些字段是创建请求时必须提供的，例如 `task_id`、`start`、`end`；
/// 有些字段只有恢复、持久化或调度后才知道，因此用 `Option<T>` 表示可选。
#[derive(Debug, Clone)]
pub struct SegmentRequest {
    pub id: Option<u32>,
    pub task_id: u32,
    pub index: u32,
    pub source_id: Option<String>,
    pub piece_start: Option<u32>,
    pub piece_end: Option<u32>,
    pub block_start: Option<u32>,
    pub block_end: Option<u32>,
    pub start: u64,
    pub end: u64,
    pub downloaded: Option<u64>,
    pub status: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub struct SegmentRequestBuilder {
    id: Option<u32>,
    task_id: u32,
    index: u32,
    source_id: Option<String>,
    piece_start: Option<u32>,
    piece_end: Option<u32>,
    block_start: Option<u32>,
    block_end: Option<u32>,
    start: u64,
    end: u64,
    downloaded: Option<u64>,
    status: Option<String>,
    updated_at: Option<DateTime<Utc>>,
}

impl SegmentRequestBuilder {
    /// 创建 builder。
    ///
    /// `new` 是关联函数，不带 `self` 参数，所以调用方式是 `SegmentRequestBuilder::new(...)`。
    /// 它只接收必要字段，其余字段用 `None` 作为默认值。
    pub fn new(task_id: u32, index: u32, start: u64, end: u64) -> Self {
        Self {
            id: None,
            task_id,
            index,
            source_id: None,
            piece_start: None,
            piece_end: None,
            block_start: None,
            block_end: None,
            start,
            end,
            downloaded: None,
            status: None,
            updated_at: None,
        }
    }

    /// builder 风格方法。
    ///
    /// `mut self` 表示这个方法取得 builder 的所有权，并允许修改它的字段。
    /// 修改完成后返回 `Self`，调用者就可以继续链式调用：
    ///
    /// ```ignore
    /// SegmentRequestBuilder::new(1, 0, 0, 1024).id(7).status("Running").build()
    /// ```
    pub fn id(mut self, id: u32) -> Self {
        self.id = Some(id);
        self
    }

    /// `impl Into<String>` 表示调用者可以传入任何能转换成 `String` 的类型。
    ///
    /// 例如 `String` 和 `&str` 都可以传入。函数内部通过 `.into()` 取得拥有所有权的 `String`。
    /// 系统解释见 `docs/rust/generics-and-traits.md`。
    pub fn source_id(mut self, source_id: impl Into<String>) -> Self {
        self.source_id = Some(source_id.into());
        self
    }

    pub fn piece_range(mut self, piece_start: u32, piece_end: u32) -> Self {
        self.piece_start = Some(piece_start);
        self.piece_end = Some(piece_end);
        self
    }

    pub fn block_range(mut self, block_start: u32, block_end: u32) -> Self {
        self.block_start = Some(block_start);
        self.block_end = Some(block_end);
        self
    }

    pub fn downloaded(mut self, downloaded: u64) -> Self {
        self.downloaded = Some(downloaded);
        self
    }

    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn updated_at(mut self, dt: DateTime<Utc>) -> Self {
        self.updated_at = Some(dt);
        self
    }

    /// 消费 builder，生成最终的 `SegmentRequest`。
    ///
    /// 这里参数是 `self` 而不是 `&self`，所以函数会取得整个 builder 的所有权。
    /// 因此可以把 `self.source_id`、`self.status` 这类字段直接移动进结果结构体，不需要 clone。
    /// 所有权细节见 `docs/rust/ownership-borrowing-lifetimes.md`。
    pub fn build(self) -> SegmentRequest {
        SegmentRequest {
            id: self.id,
            task_id: self.task_id,
            index: self.index,
            source_id: self.source_id,
            piece_start: self.piece_start,
            piece_end: self.piece_end,
            block_start: self.block_start,
            block_end: self.block_end,
            start: self.start,
            end: self.end,
            downloaded: self.downloaded,
            status: self.status,
            updated_at: self.updated_at,
        }
    }
}
