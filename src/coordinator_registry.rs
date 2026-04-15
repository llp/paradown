use crate::error::DownloadError;
use crate::manager::DownloadManager;
use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
use crate::request::{DownloadTaskRequest, DownloadWorkerRequest};
use crate::status::DownloadStatus;
use crate::task::DownloadTask;
use crate::worker::DownloadWorker;
use log::{error, warn};
use std::str::FromStr;
use std::sync::Arc;

pub(crate) async fn restore_tasks(manager: &Arc<DownloadManager>) -> Result<(), DownloadError> {
    let persistence = manager
        .persistence
        .get()
        .ok_or_else(|| DownloadError::ConfigError("Persistence not initialized".into()))?;

    let db_tasks: Vec<DBDownloadTask> = persistence.load_tasks().await?;

    for db_task in db_tasks {
        let db_workers: Vec<DBDownloadWorker> = persistence
            .load_workers(db_task.id)
            .await
            .unwrap_or_default();

        let db_checksums: Vec<DBDownloadChecksum> = persistence
            .load_checksums(db_task.id)
            .await
            .unwrap_or_default();

        let checksums = db_checksums
            .into_iter()
            .map(|c| persistence.db_to_checksum(&c))
            .collect::<Vec<_>>();

        let mut restored_status =
            DownloadStatus::from_str(&db_task.status).unwrap_or(DownloadStatus::Pending);
        restored_status = match restored_status {
            DownloadStatus::Running | DownloadStatus::Preparing => DownloadStatus::Paused,
            _ => restored_status,
        };

        let task_request = DownloadTaskRequest {
            id: Some(db_task.id),
            url: db_task.url.clone(),
            file_name: Some(db_task.file_name.clone()),
            file_path: Some(db_task.file_path.clone()),
            status: Some(restored_status),
            downloaded_size: Some(db_task.downloaded_size),
            total_size: db_task.total_size,
            checksums: Some(checksums),
            created_at: db_task.created_at,
            updated_at: db_task.updated_at,
        };

        let workers = if db_workers.is_empty() {
            None
        } else {
            Some(
                db_workers
                    .into_iter()
                    .map(|w| DownloadWorkerRequest {
                        id: Some(w.id),
                        task_id: w.task_id,
                        index: w.index,
                        start: w.start,
                        end: w.end,
                        downloaded: Some(w.downloaded),
                        status: Some(w.status.clone()),
                        updated_at: w.updated_at,
                    })
                    .collect(),
            )
        };

        if let Err(e) = add_task_with_workers(manager, task_request, workers).await {
            error!("Failed to restore task {}: {:?}", db_task.id, e);
        }
    }

    Ok(())
}

pub(crate) async fn add_task_with_workers(
    manager: &Arc<DownloadManager>,
    task_request: DownloadTaskRequest,
    workers: Option<Vec<DownloadWorkerRequest>>,
) -> Result<u32, DownloadError> {
    if let Some(existing) = manager
        .tasks
        .iter()
        .find(|entry| entry.value().url == task_request.url)
    {
        warn!(
            "[DownloadManager] Task with URL '{}' already exists, returning existing task_id {}",
            task_request.url,
            *existing.key()
        );
        return Err(DownloadError::Other(format!(
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

    let task = DownloadTask::new(
        task_id,
        task_request.url,
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
        let mut worker_vec = vec![];
        for w_req in worker_requests {
            let worker = DownloadWorker::new(
                w_req.index,
                manager.config.clone(),
                Arc::downgrade(&task),
                task.client.clone(),
                task.url.clone(),
                w_req.start,
                w_req.end,
                w_req.downloaded,
                Arc::new(task_request.file_path.clone().unwrap_or_default().into()),
                w_req
                    .status
                    .as_ref()
                    .and_then(|s| DownloadStatus::from_str(s).ok()),
                task.stats.clone(),
                w_req.updated_at,
            );
            worker_vec.push(Arc::new(worker));
        }
        let mut task_workers = task.workers.write().await;
        *task_workers = worker_vec;
    }

    task.init().await?;
    manager.tasks.insert(task_id, task);
    Ok(task_id)
}

fn next_task_id(manager: &DownloadManager, requested_id: Option<u32>) -> u32 {
    requested_id.unwrap_or_else(|| {
        let mut new_id = manager.tasks.len() as u32 + 1;
        while manager.tasks.contains_key(&new_id) {
            new_id += 1;
        }
        new_id
    })
}
