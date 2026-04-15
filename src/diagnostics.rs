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

pub(crate) async fn write_failure_diagnostic(task: &Task, error: &Error) -> Result<PathBuf, Error> {
    let diagnostics_dir = task
        .config
        .download_dir
        .join(".paradown")
        .join("diagnostics");
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
    let body = serde_json::to_vec_pretty(&diagnostic)
        .map_err(|err| Error::Other(format!("Failed to serialize diagnostic: {err}")))?;
    tokio::fs::write(&path, body).await?;

    Ok(path)
}
