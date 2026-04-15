use crate::checksum::{ChecksumAlgorithm, DownloadChecksum};
use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
use crate::request::{DownloadTaskRequest, DownloadWorkerRequest};
use crate::status::DownloadStatus;
use crate::task::DownloadTask;
use crate::worker::DownloadWorker;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) async fn task_to_db(task: &Arc<DownloadTask>) -> DBDownloadTask {
    let file_path = task
        .file_path
        .get()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let file_name = task.file_name.get().cloned().unwrap_or_default();
    let updated_at = task.updated_at.lock().await.clone();

    DBDownloadTask {
        id: task.id,
        url: task.url.clone(),
        file_name,
        file_path,
        status: task.status.lock().await.to_string(),
        downloaded_size: task.downloaded_size.load(Ordering::Relaxed),
        total_size: Some(task.total_size.load(Ordering::Relaxed)),
        created_at: task.created_at.clone(),
        updated_at,
    }
}

pub(crate) async fn worker_to_db(worker: &Arc<DownloadWorker>) -> DBDownloadWorker {
    let updated_at = worker.updated_at.lock().await.clone();

    DBDownloadWorker {
        id: worker.id,
        task_id: worker
            .task
            .upgrade()
            .map(|task| task.id)
            .unwrap_or_default(),
        index: worker.id,
        start: worker.start,
        end: worker.end,
        downloaded: worker.downloaded_size.load(Ordering::Relaxed),
        status: worker.status.lock().await.to_string(),
        updated_at,
    }
}

pub(crate) fn checksum_to_db(checksum: &DownloadChecksum, task_id: u32) -> DBDownloadChecksum {
    DBDownloadChecksum {
        id: 0,
        task_id,
        algorithm: match checksum.algorithm {
            ChecksumAlgorithm::MD5 => "MD5".to_string(),
            ChecksumAlgorithm::SHA1 => "SHA1".to_string(),
            ChecksumAlgorithm::SHA256 => "SHA256".to_string(),
            ChecksumAlgorithm::NONE => "NONE".to_string(),
        },
        value: checksum.value.clone().unwrap_or_default(),
        verified: checksum.verified.unwrap_or(false),
        verified_at: checksum.verified_at.clone(),
    }
}

pub(crate) fn db_to_checksum(model: &DBDownloadChecksum) -> DownloadChecksum {
    DownloadChecksum {
        algorithm: match model.algorithm.as_str() {
            "MD5" => ChecksumAlgorithm::MD5,
            "SHA1" => ChecksumAlgorithm::SHA1,
            "SHA256" => ChecksumAlgorithm::SHA256,
            _ => ChecksumAlgorithm::NONE,
        },
        value: Some(model.value.clone()),
        verified: Some(model.verified),
        verified_at: model.verified_at.clone(),
    }
}

pub(crate) fn db_task_to_request(
    task: &DBDownloadTask,
    checksums: &[DBDownloadChecksum],
) -> DownloadTaskRequest {
    DownloadTaskRequest {
        id: Some(task.id),
        url: task.url.clone(),
        file_name: normalized_text_field(&task.file_name),
        file_path: normalized_text_field(&task.file_path),
        checksums: Some(checksums.iter().map(db_to_checksum).collect()),
        status: Some(DownloadStatus::from_str(&task.status).unwrap_or(DownloadStatus::Pending)),
        downloaded_size: Some(task.downloaded_size),
        total_size: task.total_size,
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
    }
}

pub(crate) fn db_workers_to_requests(workers: &[DBDownloadWorker]) -> Vec<DownloadWorkerRequest> {
    workers
        .iter()
        .map(|worker| DownloadWorkerRequest {
            id: Some(worker.id),
            task_id: worker.task_id,
            index: worker.index,
            start: worker.start,
            end: worker.end,
            downloaded: Some(worker.downloaded),
            status: Some(worker.status.clone()),
            updated_at: worker.updated_at.clone(),
        })
        .collect()
}

fn normalized_text_field(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{db_task_to_request, db_workers_to_requests};
    use crate::repository::models::{DBDownloadChecksum, DBDownloadTask, DBDownloadWorker};
    use crate::status::DownloadStatus;

    #[test]
    fn normalizes_blank_task_text_fields_when_building_request() {
        let request = db_task_to_request(
            &DBDownloadTask {
                id: 7,
                url: "https://example.com/file.bin".into(),
                file_name: "   ".into(),
                file_path: "".into(),
                status: "Unknown".into(),
                downloaded_size: 12,
                total_size: Some(100),
                created_at: None,
                updated_at: None,
            },
            &[DBDownloadChecksum {
                id: 0,
                task_id: 7,
                algorithm: "SHA256".into(),
                value: "abc".into(),
                verified: false,
                verified_at: None,
            }],
        );

        assert_eq!(request.file_name, None);
        assert_eq!(request.file_path, None);
        assert!(matches!(request.status, Some(DownloadStatus::Pending)));
        assert_eq!(request.checksums.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn keeps_worker_identity_when_building_restore_requests() {
        let workers = db_workers_to_requests(&[DBDownloadWorker {
            id: 9,
            task_id: 2,
            index: 1,
            start: 50,
            end: 99,
            downloaded: 25,
            status: "Paused".into(),
            updated_at: None,
        }]);

        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].task_id, 2);
        assert_eq!(workers[0].index, 1);
        assert_eq!(workers[0].downloaded, Some(25));
        assert_eq!(workers[0].status.as_deref(), Some("Paused"));
    }
}
