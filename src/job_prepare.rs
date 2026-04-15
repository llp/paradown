use crate::config::FileConflictStrategy;
use crate::error::DownloadError;
use crate::job_finalize::{finish_job, verify_checksums};
use crate::protocol_probe::{DownloadProtocolProbe, probe_download_target};
use crate::task::DownloadTask;
use log::{debug, info};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
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

    let protocol_probe = probe_download_target(job.client.as_ref(), &job.url).await?;
    job.update_protocol_probe(
        protocol_probe.total_size,
        protocol_probe.supports_range_requests,
    );
    debug!(
        "[Task {}] Probe result: total_size={} supports_range_requests={}",
        job.id, protocol_probe.total_size, protocol_probe.supports_range_requests
    );

    if handle_existing_file(job, &file_path, protocol_probe).await? {
        return Ok(PreparationOutcome::Finished);
    }

    if protocol_probe.total_size == 0 {
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

async fn handle_existing_file(
    job: &Arc<DownloadTask>,
    file_path: &Arc<PathBuf>,
    probe: DownloadProtocolProbe,
) -> Result<bool, DownloadError> {
    if !file_path.as_ref().exists() {
        reset_missing_file_state(job).await?;
        return Ok(false);
    }

    let metadata = fs::metadata(file_path.as_ref())
        .await
        .map_err(|e| DownloadError::Io(format!("Failed to read file metadata: {}", e)))?;
    let current_size = metadata.len();

    match job.config.file_conflict_strategy {
        FileConflictStrategy::Overwrite => {
            fs::remove_file(file_path.as_ref())
                .await
                .map_err(|e| DownloadError::Io(format!("Failed to remove existing file: {}", e)))?;
            reset_resume_state(job).await?;
            debug!("[Task {}] Overwriting existing file", job.id);
            Ok(false)
        }
        FileConflictStrategy::SkipIfValid => {
            let checksum_ok = verify_existing_file(job, file_path).await?;
            if checksum_ok {
                info!("[Task {}] File exists and valid, skipping download", job.id);
                finish_job(job, Ok(())).await?;
                Ok(true)
            } else {
                debug!("[Task {}] File exists but invalid, will redownload", job.id);
                fs::remove_file(file_path.as_ref()).await?;
                reset_resume_state(job).await?;
                Ok(false)
            }
        }
        FileConflictStrategy::Resume => {
            if current_size < probe.total_size {
                if can_resume_partial_download(job, probe.supports_range_requests).await {
                    debug!(
                        "[Task {}] Resuming download from byte {}",
                        job.id, current_size
                    );
                } else {
                    info!(
                        "[Task {}] Partial file cannot be resumed safely, restarting download from scratch",
                        job.id
                    );
                    fs::remove_file(file_path.as_ref()).await?;
                    reset_resume_state(job).await?;
                }
                Ok(false)
            } else {
                let checksum_ok = verify_existing_file(job, file_path).await?;
                if checksum_ok {
                    info!("[Task {}] File exists and valid, skipping download", job.id);
                    finish_job(job, Ok(())).await?;
                    Ok(true)
                } else {
                    debug!("[Task {}] File exists but invalid, will redownload", job.id);
                    fs::remove_file(file_path.as_ref()).await?;
                    reset_resume_state(job).await?;
                    Ok(false)
                }
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

async fn can_resume_partial_download(
    job: &Arc<DownloadTask>,
    supports_range_requests: bool,
) -> bool {
    if !supports_range_requests {
        return false;
    }

    let workers = job.workers.read().await;
    !workers.is_empty()
}

async fn reset_missing_file_state(job: &Arc<DownloadTask>) -> Result<(), DownloadError> {
    let has_partial_progress = job.downloaded_size.load(Ordering::Relaxed) > 0;
    let has_workers = !job.workers.read().await.is_empty();

    if !has_partial_progress && !has_workers {
        return Ok(());
    }

    info!(
        "[Task {}] Local file is missing, clearing persisted worker progress and restarting",
        job.id
    );
    reset_resume_state(job).await
}

async fn reset_resume_state(job: &Arc<DownloadTask>) -> Result<(), DownloadError> {
    job.downloaded_size.store(0, Ordering::Relaxed);
    job.clear_task_workers().await?;
    job.purge_task_workers().await?;
    Ok(())
}
