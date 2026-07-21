use crate::checksum::{Checksum, ChecksumAlgorithm};
use crate::domain::{BlockState, DownloadSpec, HttpResourceIdentity, PieceState, SourceSet};
use crate::job::Task;
use crate::p2p::{
    TorrentEngineBackend, TorrentEngineHandle, TorrentEngineState, TorrentMetadata,
    TorrentResumeSnapshot,
};
use crate::repository::models::{
    DBDownloadBlock, DBDownloadChecksum, DBDownloadPiece, DBDownloadTask, DBDownloadWorker,
};
use crate::request::{SegmentRequest, TaskRequest};
use crate::status::Status;
use crate::worker::Worker;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// 把运行时任务转换成数据库模型。
///
/// 参数 `task: &Arc<Task>` 表示借用一个原子引用计数指针：
/// - `Arc<Task>` 允许多个异步任务共享同一个 `Task`。
/// - 这里用 `&Arc<Task>`，说明本函数只借用这个共享指针，不增加所有权负担。
///
/// 这个函数里集中出现了 `.map()`、`.unwrap_or_default()`、`.await?` 等语法，
/// 对应文档见 `docs/rust/ownership-borrowing-lifetimes.md` 和 `docs/rust/result-option.md`。
pub(crate) async fn task_to_db(task: &Arc<Task>) -> DBDownloadTask {
    let file_path = task
        .file_path
        .get()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let file_name = task.file_name.get().cloned().unwrap_or_default();
    let updated_at = *task.updated_at.lock().await;
    let resource_identity = task.http_resource_identity().await;
    let torrent_resume = task.torrent_resume_snapshot().await;
    let (
        torrent_backend,
        torrent_external_id,
        torrent_state_json,
        torrent_metadata_json,
        torrent_resume_data,
    ) = torrent_resume
        .as_ref()
        .map(torrent_resume_to_db_fields)
        .unwrap_or_default();

    DBDownloadTask {
        id: task.id,
        url: task.spec.identity_key(),
        spec_json: serde_json::to_string(&task.spec).unwrap_or_default(),
        source_set_json: serde_json::to_string(&task.source_set_snapshot().await)
            .unwrap_or_default(),
        resolved_url: resource_identity.resolved_url.unwrap_or_default(),
        entity_tag: resource_identity.entity_tag.unwrap_or_default(),
        last_modified: resource_identity.last_modified.unwrap_or_default(),
        file_name,
        file_path,
        status: task.status.lock().await.to_string(),
        downloaded_size: task.downloaded_size.load(Ordering::Relaxed),
        total_size: task.total_size_option(),
        torrent_backend,
        torrent_external_id,
        torrent_state_json,
        torrent_metadata_json,
        torrent_resume_data,
        created_at: task.created_at,
        updated_at,
    }
}

pub(crate) async fn worker_to_db(worker: &Arc<Worker>) -> DBDownloadWorker {
    let updated_at = *worker.updated_at.lock().await;
    let source = worker.current_source();
    let lane = worker.lane_snapshot();

    DBDownloadWorker {
        id: worker.id,
        task_id: worker
            .task
            .upgrade()
            .map(|task| task.id)
            .unwrap_or_default(),
        index: worker.id,
        source_id: Some(source.id),
        piece_start: Some(lane.piece_start),
        piece_end: Some(lane.piece_end),
        block_start: Some(lane.block_start),
        block_end: Some(lane.block_end),
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
    blocks: &[DBDownloadBlock],
    checksums: &[DBDownloadChecksum],
) -> TaskRequest {
    TaskRequest {
        id: Some(task.id),
        spec: serde_json::from_str::<DownloadSpec>(&task.spec_json)
            .or_else(|_| DownloadSpec::parse(task.url.clone()))
            .unwrap_or(DownloadSpec::Https {
                url: task.url.clone(),
            }),
        file_name: normalized_text_field(&task.file_name),
        file_path: normalized_text_field(&task.file_path),
        resource_identity: Some(HttpResourceIdentity {
            resolved_url: normalized_text_field(&task.resolved_url),
            entity_tag: normalized_text_field(&task.entity_tag),
            last_modified: normalized_text_field(&task.last_modified),
        }),
        http_request: None,
        sources: serde_json::from_str::<SourceSet>(&task.source_set_json).ok(),
        piece_states: Some(db_pieces_to_piece_states(pieces)),
        block_states: Some(db_blocks_to_block_states(blocks)),
        checksums: Some(checksums.iter().map(db_to_checksum).collect()),
        status: Some(Status::from_str(&task.status).unwrap_or(Status::Pending)),
        downloaded_size: Some(task.downloaded_size),
        total_size: task.total_size,
        torrent_resume: db_task_to_torrent_resume(task),
        created_at: task.created_at,
        updated_at: task.updated_at,
    }
}

type TorrentDbFields = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Vec<u8>>,
);

fn torrent_resume_to_db_fields(snapshot: &TorrentResumeSnapshot) -> TorrentDbFields {
    (
        Some(torrent_backend_to_db(snapshot.handle.backend).to_string()),
        Some(snapshot.handle.external_id.clone()),
        serde_json::to_string(&snapshot.state).ok(),
        snapshot
            .metadata
            .as_ref()
            .and_then(|metadata| serde_json::to_string(metadata).ok()),
        snapshot.resume_data.clone(),
    )
}

fn db_task_to_torrent_resume(task: &DBDownloadTask) -> Option<TorrentResumeSnapshot> {
    let backend = task
        .torrent_backend
        .as_deref()
        .and_then(torrent_backend_from_db)?;
    let external_id = normalized_text_field(task.torrent_external_id.as_deref().unwrap_or(""))?;
    let state = task
        .torrent_state_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<TorrentEngineState>(value).ok())
        .unwrap_or(TorrentEngineState::Paused);
    let metadata = task
        .torrent_metadata_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<TorrentMetadata>(value).ok());

    Some(TorrentResumeSnapshot {
        handle: TorrentEngineHandle {
            backend,
            external_id,
        },
        state,
        metadata,
        resume_data: task.torrent_resume_data.clone(),
    })
}

fn torrent_backend_to_db(backend: TorrentEngineBackend) -> &'static str {
    match backend {
        TorrentEngineBackend::Libtorrent => "libtorrent",
    }
}

fn torrent_backend_from_db(value: &str) -> Option<TorrentEngineBackend> {
    match value.trim().to_ascii_lowercase().as_str() {
        "libtorrent" => Some(TorrentEngineBackend::Libtorrent),
        _ => None,
    }
}

pub(crate) fn piece_states_to_db(
    task_id: u32,
    piece_states: &[PieceState],
) -> Vec<DBDownloadPiece> {
    // `piece_states` 是 slice 引用，函数不拥有原始集合。
    // `.iter()` 产生借用迭代器，每个 `piece` 的类型类似 `&PieceState`。
    // `.map(...)` 把每个 `PieceState` 映射成数据库模型，最后 `.collect()` 收集成 Vec。
    // 迭代器和闭包的系统解释见 `docs/rust/iterators-and-closures.md`。
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
    // `pieces` 是借用来的 slice，不能直接排序，因为排序需要可变集合。
    // `.to_vec()` 克隆出一个拥有所有权的 Vec，然后 `sort_by_key` 才能原地排序。
    let mut pieces = pieces.to_vec();
    pieces.sort_by_key(|piece| piece.piece_index);
    // 这里使用 `.into_iter()` 消费临时 Vec，把每个 DBDownloadPiece 移动进闭包。
    // 因为后面不再需要 `pieces`，消费式迭代最合适。
    pieces
        .into_iter()
        .map(|piece| PieceState {
            piece_index: piece.piece_index,
            completed: piece.completed,
        })
        .collect()
}

pub(crate) fn block_states_to_db(
    task_id: u32,
    block_states: &[BlockState],
) -> Vec<DBDownloadBlock> {
    block_states
        .iter()
        .map(|block| DBDownloadBlock {
            task_id,
            piece_index: block.piece_index,
            block_index: block.block_index,
            completed: block.completed,
            updated_at: None,
        })
        .collect()
}

pub(crate) fn db_blocks_to_block_states(blocks: &[DBDownloadBlock]) -> Vec<BlockState> {
    let mut blocks = blocks.to_vec();
    blocks.sort_by_key(|block| (block.piece_index, block.block_index));
    blocks
        .into_iter()
        .map(|block| BlockState {
            piece_index: block.piece_index,
            block_index: block.block_index,
            completed: block.completed,
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
            source_id: worker.source_id.clone(),
            piece_start: worker.piece_start,
            piece_end: worker.piece_end,
            block_start: worker.block_start,
            block_end: worker.block_end,
            start: worker.start,
            end: worker.end,
            downloaded: Some(worker.downloaded),
            status: Some(worker.status.clone()),
            updated_at: worker.updated_at,
        })
        .collect()
}

fn normalized_text_field(value: &str) -> Option<String> {
    // `trim()` 返回的是借用的 `&str`，没有分配新字符串。
    // 只有在确认非空后，才用 `to_string()` 创建拥有所有权的 `String` 放进 `Some`。
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        db_blocks_to_block_states, db_pieces_to_piece_states, db_task_to_request,
        db_workers_to_requests,
    };
    use crate::repository::models::{
        DBDownloadBlock, DBDownloadChecksum, DBDownloadPiece, DBDownloadTask, DBDownloadWorker,
    };
    use crate::status::Status;

    #[test]
    fn normalizes_blank_task_text_fields_when_building_request() {
        let request = db_task_to_request(
            &DBDownloadTask {
                id: 7,
                url: "https://example.com/file.bin".into(),
                spec_json: "".into(),
                source_set_json: "".into(),
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
                ..DBDownloadTask::default()
            },
            &[DBDownloadPiece {
                task_id: 7,
                piece_index: 0,
                completed: true,
                updated_at: None,
            }],
            &[DBDownloadBlock {
                task_id: 7,
                piece_index: 0,
                block_index: 0,
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
        assert_eq!(request.block_states.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn keeps_worker_identity_when_building_restore_requests() {
        let workers = db_workers_to_requests(&[DBDownloadWorker {
            id: 9,
            task_id: 2,
            index: 1,
            source_id: Some("source-1".into()),
            piece_start: Some(1),
            piece_end: Some(2),
            block_start: Some(3),
            block_end: Some(5),
            start: 50,
            end: 99,
            downloaded: 25,
            status: "Paused".into(),
            updated_at: None,
        }]);

        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].task_id, 2);
        assert_eq!(workers[0].index, 1);
        assert_eq!(workers[0].source_id.as_deref(), Some("source-1"));
        assert_eq!(workers[0].piece_start, Some(1));
        assert_eq!(workers[0].piece_end, Some(2));
        assert_eq!(workers[0].block_start, Some(3));
        assert_eq!(workers[0].block_end, Some(5));
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

    #[test]
    fn normalizes_block_states_from_storage_order() {
        let blocks = db_blocks_to_block_states(&[
            DBDownloadBlock {
                task_id: 1,
                piece_index: 1,
                block_index: 1,
                completed: false,
                updated_at: None,
            },
            DBDownloadBlock {
                task_id: 1,
                piece_index: 1,
                block_index: 0,
                completed: true,
                updated_at: None,
            },
        ]);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].block_index, 0);
        assert!(blocks[0].completed);
    }
}
