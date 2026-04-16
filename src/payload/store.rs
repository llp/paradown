use crate::domain::SessionManifest;
use crate::error::Error;
use crate::payload::file_map::FileMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{Mutex, RwLock};

#[derive(Debug, Clone)]
pub(crate) struct PayloadStore {
    file_map: Arc<FileMap>,
    file_handles: Arc<RwLock<HashMap<PathBuf, Arc<Mutex<File>>>>>,
}

impl PayloadStore {
    pub(crate) fn new(manifest: &SessionManifest) -> Self {
        Self {
            file_map: Arc::new(FileMap::from_manifest(manifest)),
            file_handles: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub(crate) async fn prepare(&self) -> Result<(), Error> {
        for segment in self.file_map.segments() {
            if let Some(parent) = segment.path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&segment.path)
                .await?;
            if self.file_map.is_bounded() {
                file.set_len(segment.length).await?;
            }
            self.file_handles
                .write()
                .await
                .insert(segment.path.clone(), Arc::new(Mutex::new(file)));
        }

        Ok(())
    }

    pub(crate) async fn write_at(&self, offset: u64, data: &[u8]) -> Result<(), Error> {
        let writes = self.file_map.resolve_writes(offset, data.len())?;
        for write in writes {
            let file_handle = self.file_handle(&write.path).await?;
            let mut file = file_handle.lock().await;
            file.seek(tokio::io::SeekFrom::Start(write.file_offset))
                .await?;
            file.write_all(&data[write.buffer_range]).await?;
        }

        Ok(())
    }

    async fn file_handle(&self, path: &PathBuf) -> Result<Arc<Mutex<File>>, Error> {
        if let Some(handle) = self.file_handles.read().await.get(path).cloned() {
            return Ok(handle);
        }

        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .await?;
        let handle = Arc::new(Mutex::new(file));
        let mut handles = self.file_handles.write().await;
        Ok(handles
            .entry(path.clone())
            .or_insert_with(|| Arc::clone(&handle))
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use super::PayloadStore;
    use crate::domain::{DownloadSpec, FileManifest, SessionManifest, SourceSet};
    use tempfile::TempDir;

    #[tokio::test]
    async fn writes_across_manifest_files() {
        let temp = TempDir::new().unwrap();
        let left = temp.path().join("left.bin");
        let right = temp.path().join("right.bin");

        let mut manifest = SessionManifest::for_single_file(
            DownloadSpec::parse("https://example.com/archive.bin").unwrap(),
            SourceSet::for_spec(
                &DownloadSpec::parse("https://example.com/archive.bin").unwrap(),
                None,
            ),
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
