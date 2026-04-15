use crate::checksum::{Checksum, ChecksumAlgorithm};
use crate::domain::{DownloadSpec, HttpResourceIdentity, PieceState};
use crate::job::Task;
use crate::repository::models::{
    DBDownloadChecksum, DBDownloadPiece, DBDownloadTask, DBDownloadWorker,
};
use crate::request::{SegmentRequest, TaskRequest};
use crate::status::Status;
use crate::worker::Worker;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub(crate) async fn task_to_db(task: &Arc<Task>) -> DBDownloadTask {
    let file_path = task
        .file_path
        .get()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let file_name = task.file_name.get().cloned().unwrap_or_default();
    let updated_at = *task.updated_at.lock().await;
    let resource_identity = task.http_resource_identity().await;

    DBDownloadTask {
        id: task.id,
        url: task.spec.locator().to_string(),
        resolved_url: resource_identity.resolved_url.unwrap_or_default(),
        entity_tag: resource_identity.entity_tag.unwrap_or_default(),
        last_modified: resource_identity.last_modified.unwrap_or_default(),
        file_name,
        file_path,
        status: task.status.lock().await.to_string(),
        downloaded_size: task.downloaded_size.load(Ordering::Relaxed),
        total_size: Some(task.total_size.load(Ordering::Relaxed)),
        created_at: task.created_at,
        updated_at,
    }
}

pub(crate) async fn worker_to_db(worker: &Arc<Worker>) -> DBDownloadWorker {
    let updated_at = *worker.updated_at.lock().await;

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

pub(crate) fn checksum_to_db(checksum: &Checksum, task_id: u32) -> DBDownloadChecksum {
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
        verified_at: checksum.verified_at,
    }
}

pub(crate) fn db_to_checksum(model: &DBDownloadChecksum) -> Checksum {
    Checksum {
        algorithm: match model.algorithm.as_str() {
            "MD5" => ChecksumAlgorithm::MD5,
            "SHA1" => ChecksumAlgorithm::SHA1,
            "SHA256" => ChecksumAlgorithm::SHA256,
            _ => ChecksumAlgorithm::NONE,
        },
        value: Some(model.value.clone()),
        verified: Some(model.verified),
        verified_at: model.verified_at,
    }
}

pub(crate) fn db_task_to_request(
    task: &DBDownloadTask,
    pieces: &[DBDownloadPiece],
    checksums: &[DBDownloadChecksum],
) -> TaskRequest {
    TaskRequest {
        id: Some(task.id),
        spec: DownloadSpec::parse(task.url.clone()).unwrap_or(DownloadSpec::Https {
            url: task.url.clone(),
        }),
        file_name: normalized_text_field(&task.file_name),
        file_path: normalized_text_field(&task.file_path),
        resource_identity: Some(HttpResourceIdentity {
            resolved_url: normalized_text_field(&task.resolved_url),
            entity_tag: normalized_text_field(&task.entity_tag),
            last_modified: normalized_text_field(&task.last_modified),
        }),
        piece_states: Some(db_pieces_to_piece_states(pieces)),
        checksums: Some(checksums.iter().map(db_to_checksum).collect()),
        status: Some(Status::from_str(&task.status).unwrap_or(Status::Pending)),
        downloaded_size: Some(task.downloaded_size),
        total_size: task.total_size,
        created_at: task.created_at,
        updated_at: task.updated_at,
    }
}

pub(crate) fn piece_states_to_db(
    task_id: u32,
    piece_states: &[PieceState],
) -> Vec<DBDownloadPiece> {
    piece_states
        .iter()
        .map(|piece| DBDownloadPiece {
            task_id,
            piece_index: piece.piece_index,
            completed: piece.completed,
            updated_at: None,
        })
        .collect()
}

pub(crate) fn db_pieces_to_piece_states(pieces: &[DBDownloadPiece]) -> Vec<PieceState> {
    let mut pieces = pieces.to_vec();
    pieces.sort_by_key(|piece| piece.piece_index);
    pieces
        .into_iter()
        .map(|piece| PieceState {
            piece_index: piece.piece_index,
            completed: piece.completed,
        })
        .collect()
}

pub(crate) fn db_workers_to_requests(workers: &[DBDownloadWorker]) -> Vec<SegmentRequest> {
    workers
        .iter()
        .map(|worker| SegmentRequest {
            id: Some(worker.id),
            task_id: worker.task_id,
            index: worker.index,
            start: worker.start,
            end: worker.end,
            downloaded: Some(worker.downloaded),
            status: Some(worker.status.clone()),
            updated_at: worker.updated_at,
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
    use super::{db_pieces_to_piece_states, db_task_to_request, db_workers_to_requests};
    use crate::repository::models::{
        DBDownloadChecksum, DBDownloadPiece, DBDownloadTask, DBDownloadWorker,
    };
    use crate::status::Status;

    #[test]
    fn normalizes_blank_task_text_fields_when_building_request() {
        let request = db_task_to_request(
            &DBDownloadTask {
                id: 7,
                url: "https://example.com/file.bin".into(),
                resolved_url: "".into(),
                entity_tag: "".into(),
                last_modified: "".into(),
                file_name: "   ".into(),
                file_path: "".into(),
                status: "Unknown".into(),
                downloaded_size: 12,
                total_size: Some(100),
                created_at: None,
                updated_at: None,
            },
            &[DBDownloadPiece {
                task_id: 7,
                piece_index: 0,
                completed: true,
                updated_at: None,
            }],
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
        assert_eq!(
            request
                .resource_identity
                .as_ref()
                .and_then(|identity| identity.entity_tag.clone()),
            None
        );
        assert!(matches!(request.status, Some(Status::Pending)));
        assert_eq!(request.checksums.as_ref().map(Vec::len), Some(1));
        assert_eq!(request.piece_states.as_ref().map(Vec::len), Some(1));
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

    #[test]
    fn normalizes_piece_states_from_storage_order() {
        let pieces = db_pieces_to_piece_states(&[
            DBDownloadPiece {
                task_id: 1,
                piece_index: 2,
                completed: false,
                updated_at: None,
            },
            DBDownloadPiece {
                task_id: 1,
                piece_index: 1,
                completed: true,
                updated_at: None,
            },
        ]);

        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].piece_index, 1);
        assert!(pieces[0].completed);
        assert_eq!(pieces[1].piece_index, 2);
    }
}
