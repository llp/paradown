use crate::domain::SessionManifest;
use crate::error::Error;
use crate::payload::file_map::FileMap;
use std::sync::Arc;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

#[derive(Debug, Clone)]
pub(crate) struct PayloadStore {
    file_map: Arc<FileMap>,
}

impl PayloadStore {
    pub(crate) fn new(manifest: &SessionManifest) -> Self {
        Self {
            file_map: Arc::new(FileMap::from_manifest(manifest)),
        }
    }

    pub(crate) async fn prepare(&self) -> Result<(), Error> {
        for segment in self.file_map.segments() {
            if let Some(parent) = segment.path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(&segment.path)
                .await?;
            file.set_len(segment.length).await?;
        }

        Ok(())
    }

    pub(crate) async fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), Error> {
        let writes = self.file_map.resolve_writes(offset, data.len())?;
        for write in writes {
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(&write.path)
                .await?;
            file.seek(tokio::io::SeekFrom::Start(write.file_offset))
                .await?;
            file.write_all(&data[write.buffer_range]).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PayloadStore;
    use crate::domain::{DownloadSpec, FileManifest, SessionManifest};
    use tempfile::TempDir;

    #[tokio::test]
    async fn writes_across_manifest_files() {
        let temp = TempDir::new().unwrap();
        let left = temp.path().join("left.bin");
        let right = temp.path().join("right.bin");

        let mut manifest = SessionManifest::for_single_file(
            DownloadSpec::parse("https://example.com/archive.bin").unwrap(),
            "archive.bin".into(),
            left.clone(),
            8,
            Vec::new(),
        );
        manifest.files = vec![
            FileManifest {
                path: left.clone(),
                file_name: "left.bin".into(),
                length: 4,
                offset: 0,
            },
            FileManifest {
                path: right.clone(),
                file_name: "right.bin".into(),
                length: 4,
                offset: 4,
            },
        ];

        let store = PayloadStore::new(&manifest);
        store.prepare().await.unwrap();
        store.write_at(2, b"ABCD").await.unwrap();

        assert_eq!(
            tokio::fs::read(&left).await.unwrap(),
            vec![0, 0, b'A', b'B']
        );
        assert_eq!(
            tokio::fs::read(&right).await.unwrap(),
            vec![b'C', b'D', 0, 0]
        );
    }
}
