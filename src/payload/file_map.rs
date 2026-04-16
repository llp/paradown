use crate::domain::SessionManifest;
use crate::error::Error;
use std::ops::Range;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct FileSegment {
    pub path: PathBuf,
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct WriteSlice {
    pub path: PathBuf,
    pub file_offset: u64,
    pub buffer_range: Range<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct FileMap {
    segments: Vec<FileSegment>,
    total_size: u64,
}

impl FileMap {
    pub(crate) fn from_manifest(manifest: &SessionManifest) -> Self {
        let segments = manifest
            .files
            .iter()
            .map(|file| FileSegment {
                path: file.path.clone(),
                offset: file.offset,
                length: file.length,
            })
            .collect();

        Self {
            segments,
            total_size: manifest.total_size,
        }
    }

    pub(crate) fn segments(&self) -> &[FileSegment] {
        &self.segments
    }
    pub(crate) fn resolve_writes(&self, offset: u64, len: usize) -> Result<Vec<WriteSlice>, Error> {
        let end = offset.saturating_add(len as u64);
        if end > self.total_size {
            return Err(Error::Other(format!(
                "payload write out of bounds: {}..{} exceeds {}",
                offset, end, self.total_size
            )));
        }

        let mut slices = Vec::new();
        let mut remaining_start = offset;
        let mut buffer_offset = 0usize;
        let mut remaining_len = len as u64;

        for segment in &self.segments {
            if remaining_len == 0 {
                break;
            }

            let segment_end = segment.offset.saturating_add(segment.length);
            if remaining_start >= segment_end
                || remaining_start < segment.offset && offset >= segment_end
            {
                continue;
            }

            let write_start = remaining_start.max(segment.offset);
            if write_start >= segment_end {
                continue;
            }

            let writable = (segment_end - write_start).min(remaining_len);
            let file_offset = write_start - segment.offset;
            let next_buffer_offset = buffer_offset + writable as usize;
            slices.push(WriteSlice {
                path: segment.path.clone(),
                file_offset,
                buffer_range: buffer_offset..next_buffer_offset,
            });

            remaining_start = write_start + writable;
            buffer_offset = next_buffer_offset;
            remaining_len -= writable;
        }

        if remaining_len != 0 {
            return Err(Error::Other("payload write could not be resolved".into()));
        }

        Ok(slices)
    }
}

#[cfg(test)]
mod tests {
    use super::FileMap;
    use crate::domain::{DownloadSpec, SessionManifest, SourceSet};
    use std::path::PathBuf;

    #[test]
    fn resolves_cross_file_writes() {
        let mut manifest = SessionManifest::for_single_file(
            DownloadSpec::parse("https://example.com/file.bin").unwrap(),
            SourceSet::for_spec(
                &DownloadSpec::parse("https://example.com/file.bin").unwrap(),
                None,
            ),
            "file.bin".into(),
            PathBuf::from("/tmp/a.bin"),
            8,
            Vec::new(),
        );
        manifest.files = vec![
            crate::domain::FileManifest {
                path: PathBuf::from("/tmp/a.bin"),
                file_name: "a.bin".into(),
                length: 4,
                offset: 0,
            },
            crate::domain::FileManifest {
                path: PathBuf::from("/tmp/b.bin"),
                file_name: "b.bin".into(),
                length: 4,
                offset: 4,
            },
        ];

        let file_map = FileMap::from_manifest(&manifest);
        let writes = file_map.resolve_writes(2, 4).unwrap();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0].buffer_range, 0..2);
        assert_eq!(writes[1].buffer_range, 2..4);
    }
}
