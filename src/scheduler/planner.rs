use crate::domain::SessionManifest;

const MAX_HTTP_PIECE_SIZE: u32 = 1024 * 1024;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceSelectionPolicy {
    PrimaryOnly,
    AvailabilityAware,
    RarestFirst,
    Endgame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecutionLaneAssignment {
    pub lane_id: u32,
    pub source_id: String,
    pub length_known: bool,
    pub piece_start: u32,
    pub piece_end: u32,
    pub block_start: u32,
    pub block_end: u32,
    pub start: u64,
    pub end: u64,
}

pub(crate) fn suggested_http_piece_size(total_size: u64, requested_workers: usize) -> u32 {
    if total_size == 0 {
        return 1;
    }

    let target_workers = requested_workers.max(1).min(total_size as usize);
    let ideal_piece_size = total_size.div_ceil(target_workers as u64);
    ideal_piece_size.min(MAX_HTTP_PIECE_SIZE as u64).max(1) as u32
}

pub(crate) fn plan_execution_lanes(
    manifest: &SessionManifest,
    requested_workers: usize,
    allow_parallel: bool,
) -> Vec<ExecutionLaneAssignment> {
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
    transfer_sources.sort_by(|left, right| left.id.cmp(&right.id));
    if let Some(primary) = manifest.sources.primary()
        && let Some(position) = transfer_sources
            .iter()
            .position(|source| source.id == primary.id)
    {
        transfer_sources.swap(0, position);
    }

    let policy = if allow_parallel {
        SourceSelectionPolicy::AvailabilityAware
    } else {
        SourceSelectionPolicy::PrimaryOnly
    };

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

    let worker_count = requested_workers.max(1).min(manifest.pieces.len());
    let mut assignments = Vec::with_capacity(worker_count);

    for index in 0..worker_count {
        let piece_start = (index * manifest.pieces.len()) / worker_count;
        let piece_end = ((index + 1) * manifest.pieces.len()) / worker_count;
        if piece_start == piece_end {
            continue;
        }

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

fn assignment_from_piece_slice(
    lane_id: u32,
    source_id: &str,
    pieces: &[crate::domain::PieceLayout],
    blocks: &[crate::domain::PieceBlock],
) -> Option<ExecutionLaneAssignment> {
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
        start: first.offset,
        end: last
            .offset
            .saturating_add(last.length as u64)
            .saturating_sub(1),
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
