use crate::checksum::Checksum;
use crate::domain::{DownloadSpec, PieceLayout, plan_piece_layouts};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const DEFAULT_PIECE_SIZE: u32 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifest {
    pub path: PathBuf,
    pub file_name: String,
    pub length: u64,
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManifest {
    pub id: String,
    pub spec: DownloadSpec,
    pub total_size: u64,
    pub piece_size: u32,
    pub piece_count: u32,
    pub files: Vec<FileManifest>,
    pub pieces: Vec<PieceLayout>,
    pub checksums: Vec<Checksum>,
}

impl SessionManifest {
    pub fn for_single_file(
        spec: DownloadSpec,
        file_name: String,
        file_path: PathBuf,
        total_size: u64,
        checksums: Vec<Checksum>,
    ) -> Self {
        Self::for_single_file_with_piece_size(
            spec,
            file_name,
            file_path,
            total_size,
            DEFAULT_PIECE_SIZE,
            checksums,
        )
    }

    pub fn for_single_file_with_piece_size(
        spec: DownloadSpec,
        file_name: String,
        file_path: PathBuf,
        total_size: u64,
        piece_size: u32,
        checksums: Vec<Checksum>,
    ) -> Self {
        let max_piece_size = total_size.max(1).min(u32::MAX as u64) as u32;
        let piece_size = piece_size.max(1).min(max_piece_size);
        let pieces = plan_piece_layouts(total_size, piece_size);
        let files = vec![FileManifest {
            path: file_path,
            file_name,
            length: total_size,
            offset: 0,
        }];

        Self {
            id: format!("{}::{}", spec.scheme(), spec.locator()),
            spec,
            total_size,
            piece_size,
            piece_count: pieces.len() as u32,
            files,
            pieces,
            checksums,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SessionManifest;
    use crate::domain::DownloadSpec;
    use std::path::PathBuf;

    #[test]
    fn builds_single_file_manifest() {
        let manifest = SessionManifest::for_single_file(
            DownloadSpec::parse("https://example.com/file.bin").unwrap(),
            "file.bin".into(),
            PathBuf::from("/tmp/file.bin"),
            10,
            Vec::new(),
        );

        assert_eq!(manifest.total_size, 10);
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.piece_count, 1);
        assert_eq!(manifest.files[0].file_name, "file.bin");
    }

    #[test]
    fn respects_custom_piece_size_for_single_file_manifest() {
        let manifest = SessionManifest::for_single_file_with_piece_size(
            DownloadSpec::parse("https://example.com/file.bin").unwrap(),
            "file.bin".into(),
            PathBuf::from("/tmp/file.bin"),
            10,
            4,
            Vec::new(),
        );

        assert_eq!(manifest.piece_size, 4);
        assert_eq!(manifest.piece_count, 3);
    }
}
