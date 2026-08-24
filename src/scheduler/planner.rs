use crate::domain::SessionManifest;

// 最大 HTTP Piece (分块) 尺寸限制：1 MB (1024 * 1024 字节)
const MAX_HTTP_PIECE_SIZE: u32 = 1024 * 1024;

// 源选择策略枚举
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceSelectionPolicy {
    PrimaryOnly,         // 仅使用主下载源
    AvailabilityAware,   // 感知可用性的多源分配
    RarestFirst,         // 稀缺优先（P2P / Torrent 常用）
    Endgame,             // 预备终结阶段（加速最后剩余分块）
}

/// Worker 算法执行管道分配结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionLaneAssignment {
    pub lane_id: u32,       // Worker/Lane 编号
    pub source_id: String,  // 分配的下载源 ID
    pub length_known: bool, // 是否已知总字节长度
    pub piece_start: u32,   // 分配的 Piece 起始索引
    pub piece_end: u32,     // 分配的 Piece 终止索引
    pub block_start: u32,   // 分配的 Block 起始索引
    pub block_end: u32,     // 分配的 Block 终止索引
    pub start: u64,         // 字节起始偏移量 Range Start
    pub end: u64,           // 字节终止偏移量 Range End
}

/// 步骤 6: 根据文件总大小与请求的 Worker 数量计算建议的 Piece 块大小
pub(crate) fn suggested_http_piece_size(total_size: u64, requested_workers: usize) -> u32 {
    if total_size == 0 {
        return 1;
    }

    // `requested_workers.max(1).min(total_size as usize)`: 限制目标 Worker 数量在 1 到 total_size 之间
    let target_workers = requested_workers.max(1).min(total_size as usize);

    // `div_ceil`: 向上取整除法 (Rounding-up integer division)
    let ideal_piece_size = total_size.div_ceil(target_workers as u64);

    // 限制在 1 字节 到 MAX_HTTP_PIECE_SIZE (1MB) 之间
    ideal_piece_size.min(MAX_HTTP_PIECE_SIZE as u64).max(1) as u32
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn plan_execution_lanes(
    manifest: &SessionManifest,
    requested_workers: usize,
    allow_parallel: bool,
) -> Vec<ExecutionLaneAssignment> {
    plan_execution_lanes_with_source_order(manifest, requested_workers, allow_parallel, &[])
}

/// 步骤 8: Worker 字节区间切分与源分配算法
pub(crate) fn plan_execution_lanes_with_source_order(
    manifest: &SessionManifest,
    requested_workers: usize,
    allow_parallel: bool,
    preferred_source_ids: &[String],
) -> Vec<ExecutionLaneAssignment> {
    // 若 Manifest 中没有任何 Piece 定义（如流式未知长度下载）
    if manifest.pieces.is_empty() {
        let transfer_sources = manifest.sources.active_transfer_sources();
        if !manifest.total_size_known || transfer_sources.is_empty() {
            return transfer_sources
                .first()
                .map(|source| ExecutionLaneAssignment {
                    lane_id: 0,
                    source_id: source.id.clone(),
                    length_known: false,
                    piece_start: 0,
                    piece_end: 0,
                    block_start: 0,
                    block_end: 0,
                    start: 0,
                    end: 0,
                })
                .into_iter()
                .collect();
        }
        return Vec::new();
    }

    let mut transfer_sources = manifest.sources.active_transfer_sources();
    if transfer_sources.is_empty() {
        return Vec::new();
    }
    let primary_id = manifest.sources.primary().map(|source| source.id.clone());

    // 针对可用的下载源排序：优先选择用户指定的 preferred 源或主下载源
    transfer_sources.sort_by(|left, right| {
        source_rank(
            left.id.as_str(),
            preferred_source_ids,
            primary_id.as_deref(),
        )
        .cmp(&source_rank(
            right.id.as_str(),
            preferred_source_ids,
            primary_id.as_deref(),
        ))
        // `.then_with(...)`: 如果前面的排序结果相等，继续通过字典序比较 source.id
        .then_with(|| left.id.cmp(&right.id))
    });

    let policy = if allow_parallel {
        SourceSelectionPolicy::AvailabilityAware
    } else {
        SourceSelectionPolicy::PrimaryOnly
    };

    // `matches!`: 模式匹配宏。如果是 PrimaryOnly 策略，只生成 1 个覆盖全文件的 Worker
    if matches!(policy, SourceSelectionPolicy::PrimaryOnly) {
        return vec![
            assignment_from_piece_slice(
                0,
                &transfer_sources[0].id,
                &manifest.pieces,
                &manifest.blocks,
            )
            .expect("non-empty pieces"),
        ];
    }

    // 多线程并行 Range 计算：将 manifest.pieces 切分为 worker_count 份
    let worker_count = requested_workers.max(1).min(manifest.pieces.len());
    let mut assignments = Vec::with_capacity(worker_count);

    for index in 0..worker_count {
        // 计算每个 Worker 拿到的 Piece 起止索引
        let piece_start = (index * manifest.pieces.len()) / worker_count;
        let piece_end = ((index + 1) * manifest.pieces.len()) / worker_count;
        if piece_start == piece_end {
            continue;
        }

        // 取出当前 Worker 分配到的 Piece 切片
        let slice = &manifest.pieces[piece_start..piece_end];
        let source_id = &transfer_sources[index % transfer_sources.len()].id;
        if let Some(assignment) =
            assignment_from_piece_slice(index as u32, source_id, slice, &manifest.blocks)
        {
            assignments.push(assignment);
        }
    }

    assignments
}

fn source_rank(
    source_id: &str,
    preferred_source_ids: &[String],
    primary_id: Option<&str>,
) -> (usize, usize) {
    let preferred_index = preferred_source_ids
        .iter()
        .position(|preferred| preferred == source_id)
        .unwrap_or(usize::MAX);
    let primary_rank = usize::from(primary_id != Some(source_id));
    (preferred_index, primary_rank)
}

/// 根据 Piece 切片推算字节起始与终止偏移量 (start .. end)
fn assignment_from_piece_slice(
    lane_id: u32,
    source_id: &str,
    pieces: &[crate::domain::PieceLayout],
    blocks: &[crate::domain::PieceBlock],
) -> Option<ExecutionLaneAssignment> {
    // `pieces.first()?` 与 `pieces.last()?`: 获取切片中的首个与末尾 Piece 结构
    let first = pieces.first()?;
    let last = pieces.last()?;
    let block_start = blocks
        .iter()
        .find(|block| block.piece_index == first.piece_index)
        .map(|block| block.block_index)
        .unwrap_or(0);
    let block_end = blocks
        .iter()
        .rev()
        .find(|block| block.piece_index == last.piece_index)
        .map(|block| block.block_index)
        .unwrap_or(0);

    Some(ExecutionLaneAssignment {
        lane_id,
        source_id: source_id.to_string(),
        length_known: true,
        piece_start: first.piece_index,
        piece_end: last.piece_index,
        block_start,
        block_end,
        start: first.offset, // 起始字节偏移量
        end: last
            .offset
            .saturating_add(last.length as u64)
            .saturating_sub(1), // 终止字节偏移量 (inclusive)
    })
}

#[cfg(test)]
mod tests {
    use super::{plan_execution_lanes, suggested_http_piece_size};
    use crate::domain::{DownloadSpec, SessionManifest, SourceSet};
    use std::path::PathBuf;

    #[test]
    fn suggests_piece_size_from_parallelism_goal() {
        assert_eq!(suggested_http_piece_size(100, 4), 25);
        assert_eq!(suggested_http_piece_size(3, 8), 1);
    }

    #[test]
    fn caps_suggested_piece_size_for_large_payloads() {
        assert_eq!(suggested_http_piece_size(50 * 1024 * 1024, 2), 1024 * 1024);
    }

    #[test]
    fn plans_piece_aligned_assignments_for_parallel_downloads() {
        let manifest = SessionManifest::for_single_file_with_piece_size(
            DownloadSpec::parse("https://example.com/archive.bin").unwrap(),
            SourceSet::for_spec(
                &DownloadSpec::parse("https://example.com/archive.bin").unwrap(),
                None,
            ),
            "archive.bin".into(),
            PathBuf::from("/tmp/archive.bin"),
            10,
            2,
            2,
            Vec::new(),
        );

        let assignments = plan_execution_lanes(&manifest, 2, true);
        assert_eq!(assignments.len(), 2);
        assert_eq!((assignments[0].start, assignments[0].end), (0, 3));
        assert_eq!((assignments[1].start, assignments[1].end), (4, 9));
        assert_eq!(
            (assignments[0].piece_start, assignments[0].piece_end),
            (0, 1)
        );
        assert_eq!(
            (assignments[1].piece_start, assignments[1].piece_end),
            (2, 4)
        );
    }

    #[test]
    fn falls_back_to_single_assignment_without_parallel_range_support() {
        let manifest = SessionManifest::for_single_file_with_piece_size(
            DownloadSpec::parse("https://example.com/file.bin").unwrap(),
            SourceSet::for_spec(
                &DownloadSpec::parse("https://example.com/file.bin").unwrap(),
                None,
            ),
            "file.bin".into(),
            PathBuf::from("/tmp/file.bin"),
            10,
            3,
            3,
            Vec::new(),
        );

        let assignments = plan_execution_lanes(&manifest, 4, false);
        assert_eq!(assignments.len(), 1);
        assert_eq!((assignments[0].start, assignments[0].end), (0, 9));
        assert_eq!(
            (assignments[0].piece_start, assignments[0].piece_end),
            (0, 3)
        );
    }
}
