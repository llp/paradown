use crate::coordinator::Manager;
use crate::error::Error;
use crate::job::Task;
use crate::recovery::build_restore_plan;
use crate::request::{SegmentRequest, TaskRequest};
use crate::status::Status;
use crate::worker::Worker;
use log::{error, warn};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

pub(crate) async fn restore_tasks(manager: &Arc<Manager>) -> Result<(), Error> {
    let persistence = manager
        .persistence
        .get()
        .ok_or_else(|| Error::ConfigError("Persistence not initialized".into()))?;

    let bundles = persistence.load_task_bundles().await?;

    for bundle in bundles {
        let task_id = bundle.task.id;
        let (task_request, workers) = build_restore_plan(bundle).into_parts();

        if let Err(err) = add_task_with_workers(manager, task_request, workers).await {
            error!("Failed to restore task {}: {:?}", task_id, err);
        }
    }

    Ok(())
}

pub(crate) async fn add_task_with_workers(
    manager: &Arc<Manager>,
    task_request: TaskRequest,
    workers: Option<Vec<SegmentRequest>>,
) -> Result<u32, Error> {
    let spec_identity = task_request.spec.identity_key();
    if let Some(existing) = manager
        .tasks
        .iter()
        .find(|entry| entry.value().spec.identity_key() == spec_identity)
    {
        warn!(
            "[Manager] Task with spec '{}' already exists, returning existing task_id {}",
            spec_identity,
            *existing.key()
        );
        return Err(Error::Other(format!(
            "Spec '{}' already exists!",
            spec_identity
        )));
    }

    let task_id = next_task_id(manager, task_request.id);
    let file_path = task_request
        .file_path
        .clone()
        .filter(|path| !path.trim().is_empty());
    let file_name = task_request
        .file_name
        .clone()
        .filter(|path| !path.trim().is_empty());

    let persistence = manager
        .persistence
        .get()
        .cloned()
        .expect("PersistenceManager not initialized");

    let task = Task::new(
        task_id,
        task_request.spec.clone(),
        file_name,
        file_path,
        task_request.resource_identity.clone(),
        manager
            .config
            .http
            .request
            .merged(&task_request.http_request.clone().unwrap_or_default()),
        task_request.sources.clone(),
        task_request.piece_states.clone(),
        task_request.block_states.clone(),
        task_request.status,
        task_request.downloaded_size,
        task_request.total_size,
        task_request.checksums.clone().unwrap_or_default(),
        Arc::clone(&manager.http_client),
        manager.config.clone(),
        Some(persistence),
        Arc::downgrade(manager),
        task_request.created_at,
        task_request.updated_at,
    )?;

    if let Some(worker_requests) = workers {
        let restored_workers = build_restored_workers(manager, &task, worker_requests).await;
        *task.workers.write().await = restored_workers;
    }

    task.init().await?;
    manager.tasks.insert(task_id, task);

    Ok(task_id)
}

fn next_task_id(manager: &Manager, requested_id: Option<u32>) -> u32 {
    requested_id.unwrap_or_else(|| {
        let mut new_id = manager.tasks.len() as u32 + 1;
        while manager.tasks.contains_key(&new_id) {
            new_id += 1;
        }
        new_id
    })
}

async fn build_restored_workers(
    manager: &Arc<Manager>,
    task: &Arc<Task>,
    worker_requests: Vec<SegmentRequest>,
) -> Vec<Arc<Worker>> {
    let file_path = task.file_path.get().cloned().unwrap_or_else(PathBuf::new);
    let source_set = task.source_set_snapshot().await;
    let fallback_source = source_set
        .primary()
        .cloned()
        .unwrap_or_else(|| crate::domain::SourceDescriptor::from_spec(&task.spec, None));

    worker_requests
        .into_iter()
        .map(|worker_request| {
            let source = worker_request
                .source_id
                .as_deref()
                .and_then(|source_id| source_set.get(source_id).cloned())
                .unwrap_or_else(|| fallback_source.clone());
            Arc::new(Worker::new(
                worker_request.index,
                manager.config.clone(),
                Arc::downgrade(task),
                task.client.clone(),
                task.spec.clone(),
                source.clone(),
                crate::scheduler::planner::ExecutionLaneAssignment {
                    lane_id: worker_request.index,
                    source_id: source.id.clone(),
                    length_known: true,
                    piece_start: worker_request.piece_start.unwrap_or(0),
                    piece_end: worker_request.piece_end.unwrap_or(0),
                    block_start: worker_request.block_start.unwrap_or(0),
                    block_end: worker_request.block_end.unwrap_or(0),
                    start: worker_request.start,
                    end: worker_request.end,
                },
                worker_request.downloaded,
                Arc::new(file_path.clone()),
                worker_request
                    .status
                    .as_ref()
                    .and_then(|status| Status::from_str(status).ok()),
                task.stats.clone(),
                worker_request.updated_at,
            ))
        })
        .collect()
}
