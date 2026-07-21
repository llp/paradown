use crate::domain::HttpResourceIdentity;
use crate::error::Error;
use crate::job::{Task, TaskSnapshot};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
struct FailureDiagnostic {
    generated_at: DateTime<Utc>,
    trace_id: String,
    error: String,
    task: TaskSnapshot,
    resource_identity: HttpResourceIdentity,
}

/// 写入失败诊断文件。
///
/// 这个函数适合学习 async、借用和错误传播：
/// - `async fn` 表示函数体会被编译成一个 Future。
/// - 参数 `task: &Task`、`error: &Error` 都是不可变借用，函数只读取它们。
/// - 返回 `Result<PathBuf, Error>`，成功时给出诊断文件路径，失败时返回项目统一错误。
pub(crate) async fn write_failure_diagnostic(task: &Task, error: &Error) -> Result<PathBuf, Error> {
    let diagnostics_dir = task
        .config
        .download_dir
        .join(".paradown")
        .join("diagnostics");
    // `.await?` 是两个动作：
    // 1. `.await` 等待异步文件系统操作完成。
    // 2. `?` 如果结果是 Err，就把错误转换成 `Error` 并提前返回。
    tokio::fs::create_dir_all(&diagnostics_dir).await?;

    let snapshot = task.snapshot().await;
    let resource_identity = task.http_resource_identity().await;
    let diagnostic = FailureDiagnostic {
        generated_at: Utc::now(),
        trace_id: task.trace_id().to_string(),
        error: error.to_string(),
        task: snapshot,
        resource_identity,
    };

    let path = diagnostics_dir.join(format!("task-{}.json", task.id));
    // `map_err` 把 serde_json 的错误转换成项目自己的 `Error`。
    // 如果没有这一步，`?` 需要找到从 serde_json 错误到 `Error` 的 `From` 实现。
    let body = serde_json::to_vec_pretty(&diagnostic)
        .map_err(|err| Error::Other(format!("Failed to serialize diagnostic: {err}")))?;
    tokio::fs::write(&path, body).await?;

    Ok(path)
}
