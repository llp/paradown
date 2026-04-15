#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DownloadChunk {
    pub index: u32,
    pub start: u64,
    pub end: u64,
}

pub(crate) fn plan_download_chunks(
    total_size: u64,
    requested_workers: usize,
) -> Vec<DownloadChunk> {
    if total_size == 0 {
        return Vec::new();
    }

    let worker_count = requested_workers.max(1).min(total_size as usize);
    let mut chunks = Vec::with_capacity(worker_count);

    for index in 0..worker_count {
        let start = (index as u64 * total_size) / worker_count as u64;
        let end = (((index + 1) as u64 * total_size) / worker_count as u64).saturating_sub(1);

        if start > end {
            continue;
        }

        chunks.push(DownloadChunk {
            index: index as u32,
            start,
            end,
        });
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::plan_download_chunks;

    #[test]
    fn plans_single_chunk_for_single_byte_file() {
        let chunks = plan_download_chunks(1, 4);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start, 0);
        assert_eq!(chunks[0].end, 0);
    }

    #[test]
    fn caps_worker_count_to_total_size() {
        let chunks = plan_download_chunks(3, 8);
        assert_eq!(chunks.len(), 3);
        assert_eq!((chunks[0].start, chunks[0].end), (0, 0));
        assert_eq!((chunks[1].start, chunks[1].end), (1, 1));
        assert_eq!((chunks[2].start, chunks[2].end), (2, 2));
    }

    #[test]
    fn covers_full_range_without_overlap() {
        let chunks = plan_download_chunks(10, 3);
        let ranges: Vec<(u64, u64)> = chunks.iter().map(|c| (c.start, c.end)).collect();
        assert_eq!(ranges, vec![(0, 2), (3, 5), (6, 9)]);
    }
}
