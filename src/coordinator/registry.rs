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
    if let Some(existing) = manager
        .tasks
        .iter()
        .find(|entry| entry.value().url == task_request.url)
    {
        warn!(
            "[Manager] Task with URL '{}' already exists, returning existing task_id {}",
            task_request.url,
            *existing.key()
        );
        return Err(Error::Other(format!(
            "URL '{}' already exists!",
            task_request.url
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
        task_request.url.clone(),
        file_name,
        file_path,
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
        let restored_workers =
            build_restored_workers(manager, &task, &task_request.url, worker_requests);
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

fn build_restored_workers(
    manager: &Arc<Manager>,
    task: &Arc<Task>,
    url: &str,
    worker_requests: Vec<SegmentRequest>,
) -> Vec<Arc<Worker>> {
    let file_path = task.file_path.get().cloned().unwrap_or_else(PathBuf::new);

    worker_requests
        .into_iter()
        .map(|worker_request| {
            Arc::new(Worker::new(
                worker_request.index,
                manager.config.clone(),
                Arc::downgrade(task),
                task.client.clone(),
                url.to_string(),
                worker_request.start,
                worker_request.end,
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
