use super::TorrentMetadata;
use crate::domain::{DownloadSpec, FileManifest, SessionManifest, SourceSet};
use crate::error::Error;
use std::path::{Path, PathBuf};

const TORRENT_BLOCK_SIZE: u32 = 16 * 1024;

pub(crate) fn manifest_from_torrent_metadata(
    spec: DownloadSpec,
    sources: SourceSet,
    metadata: &TorrentMetadata,
    download_dir: &Path,
    requested_file_path: Option<&Path>,
) -> Result<SessionManifest, Error> {
    let files = build_file_manifests(metadata, download_dir, requested_file_path)?;
    let block_size = metadata.piece_size.min(TORRENT_BLOCK_SIZE).max(1);

    Ok(SessionManifest::for_files_with_piece_size(
        spec,
        sources,
        metadata.stable_id(),
        metadata.total_size,
        metadata.piece_size,
        block_size,
        files,
        Vec::new(),
    ))
}

fn build_file_manifests(
    metadata: &TorrentMetadata,
    download_dir: &Path,
    requested_file_path: Option<&Path>,
) -> Result<Vec<FileManifest>, Error> {
    if metadata.files.is_empty() {
        return Err(Error::Other(
            "torrent metadata does not contain files".into(),
        ));
    }

    let mut files = Vec::with_capacity(metadata.files.len());
    for file in &metadata.files {
        let relative_path = sanitized_relative_path(&file.path_components)?;
        let path = if metadata.is_multi_file() {
            download_dir.join(&metadata.name).join(relative_path)
        } else if let Some(requested_file_path) = requested_file_path {
            requested_file_path.to_path_buf()
        } else {
            download_dir.join(relative_path)
        };

        files.push(FileManifest {
            path,
            file_name: file.file_name(),
            length: file.length,
            offset: file.offset,
        });
    }

    Ok(files)
}

fn sanitized_relative_path(components: &[String]) -> Result<PathBuf, Error> {
    if components.is_empty() {
        return Err(Error::Other("torrent file path is empty".into()));
    }

    let mut path = PathBuf::new();
    for component in components {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.contains('/')
            || component.contains('\\')
        {
            return Err(Error::Other(format!(
                "unsafe torrent path component: {component}"
            )));
        }
        path.push(component);
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{manifest_from_torrent_metadata, sanitized_relative_path};
    use crate::domain::{DownloadSpec, SourceSet};
    use crate::p2p::{TorrentFileEntry, TorrentMetadata, TorrentPieceHash};
    use std::path::{Path, PathBuf};

    fn metadata() -> TorrentMetadata {
        TorrentMetadata {
            name: "bundle".into(),
            info_hash_v1: Some("0123456789abcdef0123456789abcdef01234567".into()),
            info_hash_v2: None,
            piece_size: 4,
            piece_count: 3,
            total_size: 10,
            private: false,
            files: vec![
                TorrentFileEntry {
                    path_components: vec!["a.bin".into()],
                    length: 4,
                    offset: 0,
                },
                TorrentFileEntry {
                    path_components: vec!["nested".into(), "b.bin".into()],
                    length: 6,
                    offset: 4,
                },
            ],
            piece_hashes: vec![TorrentPieceHash {
                piece_index: 0,
                sha1: Some("hash".into()),
                sha256: None,
            }],
            trackers: Vec::new(),
            web_seeds: Vec::new(),
        }
    }

    #[test]
    fn rejects_unsafe_torrent_path_components() {
        let err = sanitized_relative_path(&["..".into(), "payload.bin".into()]).unwrap_err();
        assert!(err.to_string().contains("unsafe torrent path component"));
    }

    #[test]
    fn maps_torrent_metadata_into_session_manifest() {
        let spec = DownloadSpec::parse("/tmp/bundle.torrent").unwrap();
        let manifest = manifest_from_torrent_metadata(
            spec.clone(),
            SourceSet::for_spec(&spec, None),
            &metadata(),
            Path::new("/downloads"),
            None,
        )
        .unwrap();

        assert_eq!(manifest.id, "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(manifest.total_size, 10);
        assert_eq!(manifest.piece_size, 4);
        assert_eq!(manifest.piece_count, 3);
        assert_eq!(manifest.block_size, 4);
        assert_eq!(manifest.files.len(), 2);
        assert_eq!(
            manifest.files[0].path,
            PathBuf::from("/downloads/bundle/a.bin")
        );
        assert_eq!(
            manifest.files[1].path,
            PathBuf::from("/downloads/bundle/nested/b.bin")
        );
    }
}
