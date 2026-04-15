use crate::error::DownloadError;
use crate::manager::DownloadManager;
use crate::persistence::{DownloadPersistenceManager, StoredDownloadBundle};
use crate::request::{DownloadTaskRequest, DownloadWorkerRequest};
use crate::status::DownloadStatus;
use crate::task::DownloadTask;
use crate::worker::DownloadWorker;
use log::{error, warn};
use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

pub(crate) async fn restore_tasks(manager: &Arc<DownloadManager>) -> Result<(), DownloadError> {
    let persistence = manager
        .persistence
        .get()
        .ok_or_else(|| DownloadError::ConfigError("Persistence not initialized".into()))?;

    let bundles = persistence.load_task_bundles().await?;

    for bundle in bundles {
        let task_id = bundle.task.id;
        let restore_plan = build_restore_plan(persistence, bundle);

        if let Err(err) =
            add_task_with_workers(manager, restore_plan.task_request, restore_plan.workers).await
        {
            error!("Failed to restore task {}: {:?}", task_id, err);
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

#[derive(Debug)]
struct RestorePlan {
    task_request: DownloadTaskRequest,
    workers: Option<Vec<DownloadWorkerRequest>>,
}

fn build_restore_plan(
    persistence: &DownloadPersistenceManager,
    bundle: StoredDownloadBundle,
) -> RestorePlan {
    let StoredDownloadBundle {
        task,
        workers,
        checksums,
    } = bundle;
    let mut task_request = persistence.db_task_to_request(&task, &checksums);
    let worker_requests = persistence.db_workers_to_requests(&workers);

    let mut restored_status = normalize_restored_status(
        task_request
            .status
            .clone()
            .unwrap_or(DownloadStatus::Pending),
    );
    let file_state = inspect_restore_file_state(task_request.file_path.as_deref());

    if matches!(restored_status, DownloadStatus::Completed)
        && !file_state.is_usable_for_completed(task_request.total_size)
    {
        warn!(
            "[Restore Task {}] Completed record has no usable local file, downgrading to Pending",
            task.id
        );
        restored_status = DownloadStatus::Pending;
    }

    let sanitized_workers =
        if matches!(restored_status, DownloadStatus::Paused) && file_state.exists {
            sanitize_worker_requests(task.id, task_request.total_size, worker_requests)
        } else {
            Vec::new()
        };

    if matches!(restored_status, DownloadStatus::Paused) && sanitized_workers.is_empty() {
        warn!(
            "[Restore Task {}] No trustworthy worker state found, downgrading to Pending",
            task.id
        );
        restored_status = DownloadStatus::Pending;
    }

    task_request.status = Some(restored_status);

    if !sanitized_workers.is_empty() {
        let downloaded_size = sanitized_workers
            .iter()
            .map(|worker| worker.downloaded.unwrap_or(0))
            .sum();
        task_request.downloaded_size = Some(downloaded_size);

        if task_request.total_size.is_none() || task_request.total_size == Some(0) {
            task_request.total_size = sanitized_workers
                .iter()
                .map(|worker| worker.end.saturating_add(1))
                .max();
        }
    } else if !file_state.exists || !matches!(task_request.status, Some(DownloadStatus::Completed))
    {
        task_request.downloaded_size = Some(0);
    }

    RestorePlan {
        task_request,
        workers: if sanitized_workers.is_empty() {
            None
        } else {
            Some(sanitized_workers)
        },
    }
}

fn normalize_restored_status(status: DownloadStatus) -> DownloadStatus {
    match status {
        DownloadStatus::Running | DownloadStatus::Preparing => DownloadStatus::Paused,
        other => other,
    }
}

fn sanitize_worker_requests(
    task_id: u32,
    total_size: Option<u64>,
    mut workers: Vec<DownloadWorkerRequest>,
) -> Vec<DownloadWorkerRequest> {
    workers.sort_by_key(|worker| worker.index);

    let mut sanitized = Vec::with_capacity(workers.len());
    let mut seen_indexes = HashSet::new();
    let mut expected_start = 0;

    for mut worker in workers {
        if worker.task_id != task_id {
            warn!(
                "[Restore Task {}] Rejecting worker layout because worker {} has mismatched task_id {}",
                task_id, worker.index, worker.task_id
            );
            return Vec::new();
        }

        if !seen_indexes.insert(worker.index) {
            warn!(
                "[Restore Task {}] Rejecting worker layout because index {} is duplicated",
                task_id, worker.index
            );
            return Vec::new();
        }

        if worker.start > worker.end {
            warn!(
                "[Restore Task {}] Rejecting worker layout because worker {} range {}-{} is invalid",
                task_id, worker.index, worker.start, worker.end
            );
            return Vec::new();
        }

        let expected_length = worker.end.saturating_sub(worker.start).saturating_add(1);
        let downloaded = worker.downloaded.unwrap_or(0);
        if downloaded > expected_length {
            warn!(
                "[Restore Task {}] Rejecting worker layout because worker {} downloaded {} exceeds chunk length {}",
                task_id, worker.index, downloaded, expected_length
            );
            return Vec::new();
        }

        if let Some(total_size) = total_size {
            if total_size > 0 && worker.end >= total_size {
                warn!(
                    "[Restore Task {}] Rejecting worker layout because worker {} range end {} exceeds total_size {}",
                    task_id, worker.index, worker.end, total_size
                );
                return Vec::new();
            }
        }

        if worker.start != expected_start {
            warn!(
                "[Restore Task {}] Rejecting worker layout because worker {} starts at {}, expected {}",
                task_id, worker.index, worker.start, expected_start
            );
            return Vec::new();
        }

        worker.status = normalize_worker_status(worker.status);
        expected_start = worker.end.saturating_add(1);
        sanitized.push(worker);
    }

    if let Some(total_size) = total_size {
        if total_size > 0 && expected_start != total_size {
            warn!(
                "[Restore Task {}] Rejecting worker layout because restored ranges end at {}, expected total_size {}",
                task_id, expected_start, total_size
            );
            return Vec::new();
        }
    }

    sanitized
}

fn normalize_worker_status(status: Option<String>) -> Option<String> {
    match status.as_deref() {
        Some("Running") | Some("Preparing") => Some("Paused".into()),
        Some("Deleted") => Some("Pending".into()),
        Some(other) => Some(other.to_string()),
        None => Some("Pending".into()),
    }
}

#[derive(Debug, Clone, Copy)]
struct RestoreFileState {
    exists: bool,
    size: Option<u64>,
}

impl RestoreFileState {
    fn is_usable_for_completed(self, total_size: Option<u64>) -> bool {
        if !self.exists {
            return false;
        }

        match (self.size, total_size) {
            (Some(size), Some(total)) if total > 0 => size == total,
            (Some(_), _) => true,
            _ => false,
        }
    }
}

fn inspect_restore_file_state(file_path: Option<&str>) -> RestoreFileState {
    let Some(file_path) = file_path.filter(|path| !path.trim().is_empty()) else {
        return RestoreFileState {
            exists: false,
            size: None,
        };
    };

    let path = Path::new(file_path);
    if !path.exists() {
        return RestoreFileState {
            exists: false,
            size: None,
        };
    }

    let size = std::fs::metadata(path).ok().map(|metadata| metadata.len());
    RestoreFileState { exists: true, size }
}

#[cfg(test)]
mod tests {
    use super::build_restore_plan;
    use crate::persistence::DownloadPersistenceManager;
    use crate::persistence::StoredDownloadBundle;
    use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
    use crate::status::DownloadStatus;
    use chrono::{TimeZone, Utc};
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn downgrades_paused_restore_without_local_file() {
        let persistence = test_persistence().await;
        let plan = build_restore_plan(
            &persistence,
            StoredDownloadBundle {
                task: DBDownloadTask {
                    id: 1,
                    url: "https://example.com/file.bin".into(),
                    file_name: "file.bin".into(),
                    file_path: "/tmp/missing-file.bin".into(),
                    status: "Running".into(),
                    downloaded_size: 50,
                    total_size: Some(100),
                    created_at: None,
                    updated_at: None,
                },
                workers: vec![DBDownloadWorker {
                    id: 1,
                    task_id: 1,
                    index: 0,
                    start: 0,
                    end: 99,
                    downloaded: 50,
                    status: "Running".into(),
                    updated_at: None,
                }],
                checksums: vec![],
            },
        );

        assert!(plan.workers.is_none());
        assert!(matches!(
            plan.task_request.status,
            Some(DownloadStatus::Pending)
        ));
        assert_eq!(plan.task_request.downloaded_size, Some(0));
    }

    #[tokio::test]
    async fn keeps_valid_paused_restore_and_recomputes_progress() {
        let persistence = test_persistence().await;
        let file = NamedTempFile::new().unwrap();
        let file_path = file.path().to_string_lossy().to_string();

        let plan = build_restore_plan(
            &persistence,
            StoredDownloadBundle {
                task: DBDownloadTask {
                    id: 2,
                    url: "https://example.com/archive.iso".into(),
                    file_name: "archive.iso".into(),
                    file_path: file_path.clone(),
                    status: "Paused".into(),
                    downloaded_size: 1,
                    total_size: Some(100),
                    created_at: None,
                    updated_at: None,
                },
                workers: vec![
                    DBDownloadWorker {
                        id: 1,
                        task_id: 2,
                        index: 0,
                        start: 0,
                        end: 49,
                        downloaded: 50,
                        status: "Completed".into(),
                        updated_at: None,
                    },
                    DBDownloadWorker {
                        id: 2,
                        task_id: 2,
                        index: 1,
                        start: 50,
                        end: 99,
                        downloaded: 10,
                        status: "Running".into(),
                        updated_at: None,
                    },
                ],
                checksums: vec![DBDownloadChecksum {
                    id: 0,
                    task_id: 2,
                    algorithm: "SHA256".into(),
                    value: "abc".into(),
                    verified: false,
                    verified_at: None,
                }],
            },
        );

        assert!(matches!(
            plan.task_request.status,
            Some(DownloadStatus::Paused)
        ));
        assert_eq!(plan.task_request.downloaded_size, Some(60));
        assert_eq!(plan.workers.as_ref().map(Vec::len), Some(2));
        assert_eq!(
            plan.workers.as_ref().unwrap()[1].status.as_deref(),
            Some("Paused")
        );
    }

    #[tokio::test]
    async fn drops_invalid_worker_layout_and_falls_back_to_pending() {
        let persistence = test_persistence().await;
        let file = NamedTempFile::new().unwrap();
        let file_path = file.path().to_string_lossy().to_string();

        let plan = build_restore_plan(
            &persistence,
            StoredDownloadBundle {
                task: DBDownloadTask {
                    id: 3,
                    url: "https://example.com/video.mp4".into(),
                    file_name: "video.mp4".into(),
                    file_path,
                    status: "Paused".into(),
                    downloaded_size: 80,
                    total_size: Some(100),
                    created_at: Some(Utc.with_ymd_and_hms(2026, 4, 15, 8, 0, 0).unwrap()),
                    updated_at: Some(Utc.with_ymd_and_hms(2026, 4, 15, 8, 30, 0).unwrap()),
                },
                workers: vec![
                    DBDownloadWorker {
                        id: 1,
                        task_id: 3,
                        index: 0,
                        start: 0,
                        end: 59,
                        downloaded: 60,
                        status: "Completed".into(),
                        updated_at: None,
                    },
                    DBDownloadWorker {
                        id: 2,
                        task_id: 3,
                        index: 1,
                        start: 40,
                        end: 99,
                        downloaded: 20,
                        status: "Paused".into(),
                        updated_at: None,
                    },
                ],
                checksums: vec![],
            },
        );

        assert!(plan.workers.is_none());
        assert!(matches!(
            plan.task_request.status,
            Some(DownloadStatus::Pending)
        ));
        assert_eq!(plan.task_request.downloaded_size, Some(0));
    }

    async fn test_persistence() -> DownloadPersistenceManager {
        let config = crate::config::DownloadConfigBuilder::new()
            .persistence_type(crate::persistence::PersistenceType::Memory)
            .build()
            .unwrap();
        DownloadPersistenceManager::new(Arc::new(config))
            .await
            .unwrap()
    }
}
