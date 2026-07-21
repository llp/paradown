use crate::Error;
use crate::p2p::TorrentDiagnosticEvent;

/// 任务运行过程中向外发出的事件。
///
/// 这个 enum 是学习 enum 变体形式的好例子：
/// - `Pending(u32)` 是元组风格变体，用位置保存字段。
/// - `Progress { id, downloaded, total }` 是结构体风格变体，用字段名保存数据。
/// - `Cancel(u32)` 这类变体把事件类型和任务 id 放在一起。
///
/// 选择哪种变体风格通常取决于可读性：
/// - 字段少且含义明显时，元组风格更短。
/// - 字段多或同类型字段容易混淆时，结构体风格更清楚。
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
