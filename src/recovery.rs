use crate::request::{SegmentRequest, TaskRequest};
use crate::status::Status;
use crate::storage::StoredBundle;
use crate::storage::mapping::{db_task_to_request, db_workers_to_requests};
use log::warn;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug)]
pub(crate) struct RestorePlan {
    task_request: TaskRequest,
    workers: Option<Vec<SegmentRequest>>,
}

impl RestorePlan {
    pub(crate) fn into_parts(self) -> (TaskRequest, Option<Vec<SegmentRequest>>) {
        (self.task_request, self.workers)
    }
}

pub(crate) fn build_restore_plan(bundle: StoredBundle) -> RestorePlan {
    let StoredBundle {
        task,
        workers,
        pieces,
        checksums,
    } = bundle;
    let mut task_request = db_task_to_request(&task, &pieces, &checksums);
    let worker_requests = db_workers_to_requests(&workers);

    let mut restored_status =
        normalize_restored_status(task_request.status.clone().unwrap_or(Status::Pending));
    let file_state = inspect_restore_file_state(task_request.file_path.as_deref());

    if matches!(restored_status, Status::Completed)
        && !file_state.is_usable_for_completed(task_request.total_size)
    {
        warn!(
            "[Restore Task {}] Completed record has no usable local file, downgrading to Pending",
            task.id
        );
        restored_status = Status::Pending;
        task_request.piece_states = Some(Vec::new());
    }

    let sanitized_workers = if matches!(restored_status, Status::Paused) && file_state.exists {
        sanitize_worker_requests(task.id, task_request.total_size, worker_requests)
    } else {
        Vec::new()
    };

    if matches!(restored_status, Status::Paused)
        && sanitized_workers.is_empty()
        && !has_restorable_piece_state(&task_request)
    {
        warn!(
            "[Restore Task {}] No trustworthy worker state found, downgrading to Pending",
            task.id
        );
        restored_status = Status::Pending;
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
    } else if !file_state.exists
        || (!matches!(task_request.status, Some(Status::Completed))
            && !has_restorable_piece_state(&task_request))
    {
        task_request.downloaded_size = Some(0);
    }

    if !file_state.exists {
        task_request.piece_states = Some(Vec::new());
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

fn has_restorable_piece_state(task_request: &TaskRequest) -> bool {
    task_request
        .piece_states
        .as_ref()
        .is_some_and(|pieces| pieces.iter().any(|piece| piece.completed))
}

fn normalize_restored_status(status: Status) -> Status {
    match status {
        Status::Running | Status::Preparing => Status::Paused,
        other => other,
    }
}

fn sanitize_worker_requests(
    task_id: u32,
    total_size: Option<u64>,
    mut workers: Vec<SegmentRequest>,
) -> Vec<SegmentRequest> {
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
    use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
    use crate::status::Status;
    use crate::storage::StoredBundle;
    use chrono::{TimeZone, Utc};
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn downgrades_paused_restore_without_local_file() {
        let plan = build_restore_plan(StoredBundle {
            task: DBDownloadTask {
                id: 1,
                url: "https://example.com/file.bin".into(),
                resolved_url: "".into(),
                entity_tag: "".into(),
                last_modified: "".into(),
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
            pieces: vec![],
            checksums: vec![],
        });

        let (task_request, workers) = plan.into_parts();
        assert!(workers.is_none());
        assert!(matches!(task_request.status, Some(Status::Pending)));
        assert_eq!(task_request.downloaded_size, Some(0));
    }

    #[test]
    fn keeps_valid_paused_restore_and_recomputes_progress() {
        let file = NamedTempFile::new().unwrap();
        let file_path = file.path().to_string_lossy().to_string();

        let plan = build_restore_plan(StoredBundle {
            task: DBDownloadTask {
                id: 2,
                url: "https://example.com/archive.iso".into(),
                resolved_url: "".into(),
                entity_tag: "".into(),
                last_modified: "".into(),
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
            pieces: vec![],
            checksums: vec![DBDownloadChecksum {
                id: 0,
                task_id: 2,
                algorithm: "SHA256".into(),
                value: "abc".into(),
                verified: false,
                verified_at: None,
            }],
        });

        let (task_request, workers) = plan.into_parts();
        assert!(matches!(task_request.status, Some(Status::Paused)));
        assert_eq!(task_request.downloaded_size, Some(60));
        assert_eq!(workers.as_ref().map(Vec::len), Some(2));
        assert_eq!(
            workers.as_ref().unwrap()[1].status.as_deref(),
            Some("Paused")
        );
    }

    #[test]
    fn drops_invalid_worker_layout_and_falls_back_to_pending() {
        let file = NamedTempFile::new().unwrap();
        let file_path = file.path().to_string_lossy().to_string();

        let plan = build_restore_plan(StoredBundle {
            task: DBDownloadTask {
                id: 3,
                url: "https://example.com/video.mp4".into(),
                resolved_url: "".into(),
                entity_tag: "".into(),
                last_modified: "".into(),
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
            pieces: vec![],
            checksums: vec![],
        });

        let (task_request, workers) = plan.into_parts();
        assert!(workers.is_none());
        assert!(matches!(task_request.status, Some(Status::Pending)));
        assert_eq!(task_request.downloaded_size, Some(0));
    }

    #[test]
    fn downgrades_completed_restore_when_local_file_size_is_wrong() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"too-short").unwrap();

        let plan = build_restore_plan(StoredBundle {
            task: DBDownloadTask {
                id: 4,
                url: "https://example.com/release.tar".into(),
                resolved_url: "".into(),
                entity_tag: "".into(),
                last_modified: "".into(),
                file_name: "release.tar".into(),
                file_path: file.path().to_string_lossy().to_string(),
                status: "Completed".into(),
                downloaded_size: 128,
                total_size: Some(128),
                created_at: None,
                updated_at: None,
            },
            workers: vec![],
            pieces: vec![],
            checksums: vec![],
        });

        let (task_request, workers) = plan.into_parts();
        assert!(workers.is_none());
        assert!(matches!(task_request.status, Some(Status::Pending)));
        assert_eq!(task_request.downloaded_size, Some(0));
    }

    #[test]
    fn keeps_paused_restore_when_piece_state_can_rebuild_progress() {
        let file = NamedTempFile::new().unwrap();
        let file_path = file.path().to_string_lossy().to_string();

        let plan = build_restore_plan(StoredBundle {
            task: DBDownloadTask {
                id: 5,
                url: "https://example.com/pieces.bin".into(),
                resolved_url: "".into(),
                entity_tag: "\"etag-a\"".into(),
                last_modified: "".into(),
                file_name: "pieces.bin".into(),
                file_path,
                status: "Paused".into(),
                downloaded_size: 32,
                total_size: Some(64),
                created_at: None,
                updated_at: None,
            },
            workers: vec![],
            pieces: vec![crate::repository::models::DBDownloadPiece {
                task_id: 5,
                piece_index: 0,
                completed: true,
                updated_at: None,
            }],
            checksums: vec![],
        });

        let (task_request, workers) = plan.into_parts();
        assert!(workers.is_none());
        assert!(matches!(task_request.status, Some(Status::Paused)));
        assert_eq!(task_request.downloaded_size, Some(32));
        assert_eq!(task_request.piece_states.as_ref().map(Vec::len), Some(1));
        assert!(task_request.piece_states.unwrap()[0].completed);
    }
}
