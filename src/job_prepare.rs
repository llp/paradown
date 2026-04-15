use crate::config::FileConflictStrategy;
use crate::error::DownloadError;
use crate::job_finalize::{finish_job, verify_checksums};
use crate::task::DownloadTask;
use log::{debug, info};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;

pub(crate) struct PreparedDownload {
    pub(crate) file_path: Arc<PathBuf>,
}

pub(crate) enum PreparationOutcome {
    Ready(PreparedDownload),
    Finished,
}

pub(crate) async fn prepare_download(
    job: &Arc<DownloadTask>,
) -> Result<PreparationOutcome, DownloadError> {
    let download_dir = &job.config.download_dir;
    if !download_dir.exists() {
        fs::create_dir_all(download_dir).await.map_err(|e| {
            DownloadError::Io(format!("Failed to create download directory: {}", e))
        })?;
        debug!(
            "[Task {}] Created download directory: {}",
            job.id,
            download_dir.display()
        );
    }

    let _ = job.resolve_or_init_file_name();
    let file_path = job.get_or_init_file_path(download_dir)?;
    debug!("[Task {}] Download file path: {:?}", job.id, file_path);

    let total_size = probe_total_size(job).await?;
    debug!("[Task {}] Total file size: {} bytes", job.id, total_size);

    if handle_existing_file(job, &file_path, total_size).await? {
        return Ok(PreparationOutcome::Finished);
    }

    if total_size == 0 {
        tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(file_path.as_ref())
            .await?;

        let outcome = match verify_checksums(job, file_path.as_ref()).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(DownloadError::Other("Checksum failed".into())),
            Err(err) => Err(err),
        };
        finish_job(job, outcome).await?;
        return Ok(PreparationOutcome::Finished);
    }

    Ok(PreparationOutcome::Ready(PreparedDownload { file_path }))
}

async fn probe_total_size(job: &Arc<DownloadTask>) -> Result<u64, DownloadError> {
    let known_size = job.total_size.load(std::sync::atomic::Ordering::Relaxed);
    if known_size > 0 {
        return Ok(known_size);
    }

    let resp = job.client.head(&job.url).send().await?;
    if !resp.status().is_success() {
        return Err(format!("Failed to fetch file info: {}", resp.status()).into());
    }

    let size = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .ok_or("No Content-Length")?
        .to_str()?
        .parse::<u64>()?;
    job.total_size
        .store(size, std::sync::atomic::Ordering::Relaxed);
    Ok(size)
}

async fn handle_existing_file(
    job: &Arc<DownloadTask>,
    file_path: &Arc<PathBuf>,
    total_size: u64,
) -> Result<bool, DownloadError> {
    if !file_path.as_ref().exists() {
        return Ok(false);
    }

    let metadata = fs::metadata(file_path.as_ref())
        .await
        .map_err(|e| DownloadError::Io(format!("Failed to read file metadata: {}", e)))?;
    let current_size = metadata.len();
    let checksum_ok = verify_existing_file(job, file_path).await?;

    match job.config.file_conflict_strategy {
        FileConflictStrategy::Overwrite => {
            fs::remove_file(file_path.as_ref())
                .await
                .map_err(|e| DownloadError::Io(format!("Failed to remove existing file: {}", e)))?;
            debug!("[Task {}] Overwriting existing file", job.id);
            Ok(false)
        }
        FileConflictStrategy::SkipIfValid => {
            if checksum_ok {
                info!("[Task {}] File exists and valid, skipping download", job.id);
                finish_job(job, Ok(())).await?;
                Ok(true)
            } else {
                debug!("[Task {}] File exists but invalid, will redownload", job.id);
                fs::remove_file(file_path.as_ref()).await?;
                Ok(false)
            }
        }
        FileConflictStrategy::Resume => {
            if current_size < total_size {
                debug!(
                    "[Task {}] Resuming download from byte {}",
                    job.id, current_size
                );
                Ok(false)
            } else if checksum_ok {
                info!("[Task {}] File exists and valid, skipping download", job.id);
                finish_job(job, Ok(())).await?;
                Ok(true)
            } else {
                debug!("[Task {}] File exists but invalid, will redownload", job.id);
                fs::remove_file(file_path.as_ref()).await?;
                Ok(false)
            }
        }
    }
}

async fn verify_existing_file(
    job: &Arc<DownloadTask>,
    file_path: &Arc<PathBuf>,
) -> Result<bool, DownloadError> {
    let mut checksums = job.checksums.lock().await;

    for checksum in checksums.iter_mut() {
        if checksum.verified.unwrap_or(false) {
            continue;
        }

        match checksum.verify(file_path.as_ref()) {
            Ok(true) => {
                checksum.verified = Some(true);
                checksum.verified_at = Some(chrono::Utc::now());
            }
            Ok(false) => return Ok(false),
            Err(err) => return Err(err),
        }
    }

    Ok(true)
}
