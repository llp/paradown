use crate::domain::SessionManifest;

const MAX_HTTP_PIECE_SIZE: u32 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PieceAssignment {
    pub index: u32,
    pub piece_start: u32,
    pub piece_end: u32,
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

pub(crate) fn plan_piece_assignments(
    manifest: &SessionManifest,
    requested_workers: usize,
    allow_parallel: bool,
) -> Vec<PieceAssignment> {
    if manifest.pieces.is_empty() {
        return Vec::new();
    }

    if !allow_parallel {
        return vec![assignment_from_piece_slice(0, &manifest.pieces).expect("non-empty pieces")];
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
        if let Some(assignment) = assignment_from_piece_slice(index as u32, slice) {
            assignments.push(assignment);
        }
    }

    assignments
}

fn assignment_from_piece_slice(
    index: u32,
    pieces: &[crate::domain::PieceLayout],
) -> Option<PieceAssignment> {
    let first = pieces.first()?;
    let last = pieces.last()?;

    Some(PieceAssignment {
        index,
        piece_start: first.piece_index,
        piece_end: last.piece_index,
        start: first.offset,
        end: last
            .offset
            .saturating_add(last.length as u64)
            .saturating_sub(1),
    })
}

#[cfg(test)]
mod tests {
    use super::{plan_piece_assignments, suggested_http_piece_size};
    use crate::domain::{DownloadSpec, SessionManifest};
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
            "archive.bin".into(),
            PathBuf::from("/tmp/archive.bin"),
            10,
            2,
            Vec::new(),
        );

        let assignments = plan_piece_assignments(&manifest, 2, true);
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
            "file.bin".into(),
            PathBuf::from("/tmp/file.bin"),
            10,
            3,
            Vec::new(),
        );

        let assignments = plan_piece_assignments(&manifest, 4, false);
        assert_eq!(assignments.len(), 1);
        assert_eq!((assignments[0].start, assignments[0].end), (0, 9));
        assert_eq!(
            (assignments[0].piece_start, assignments[0].piece_end),
            (0, 3)
        );
    }
}
