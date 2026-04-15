use crate::config::FileConflictStrategy;
use crate::discovery::origin::{OriginMetadata, discover_origin};
use crate::domain::SessionManifest;
use crate::error::Error;
use crate::job::Task;
use crate::job::finalize::{finish_job, verify_checksums};
use crate::payload::verifier::verify_file_checksums;
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

pub(crate) async fn prepare_download(job: &Arc<Task>) -> Result<PreparationOutcome, Error> {
    let download_dir = &job.config.download_dir;
    if !download_dir.exists() {
        fs::create_dir_all(download_dir)
            .await
            .map_err(|e| Error::Io(format!("Failed to create download directory: {}", e)))?;
        debug!(
            "[Task {}] Created download directory: {}",
            job.id,
            download_dir.display()
        );
    }

    let _ = job.resolve_or_init_file_name();
    let file_path = job.get_or_init_file_path(download_dir)?;
    debug!("[Task {}] Download file path: {:?}", job.id, file_path);

    let protocol_probe = discover_origin(job.client.as_ref(), &job.spec).await?;
    job.update_protocol_probe(
        protocol_probe.total_size,
        protocol_probe.supports_range_requests,
    );
    debug!(
        "[Task {}] Probe result: total_size={} supports_range_requests={}",
        job.id, protocol_probe.total_size, protocol_probe.supports_range_requests
    );

    let manifest = build_single_file_manifest(job, &file_path, protocol_probe.total_size).await;
    let payload_store = job.install_manifest(manifest).await;

    if handle_existing_file(job, &file_path, protocol_probe).await? {
        return Ok(PreparationOutcome::Finished);
    }

    payload_store.prepare().await?;

    if protocol_probe.total_size == 0 {
        let outcome = match verify_checksums(job, file_path.as_ref()).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::Other("Checksum failed".into())),
            Err(err) => Err(err),
        };
        finish_job(job, outcome).await?;
        return Ok(PreparationOutcome::Finished);
    }

    Ok(PreparationOutcome::Ready(PreparedDownload { file_path }))
}

async fn build_single_file_manifest(
    job: &Arc<Task>,
    file_path: &Arc<PathBuf>,
    total_size: u64,
) -> SessionManifest {
    let checksums = job.checksums.lock().await.clone();
    SessionManifest::for_single_file(
        job.spec.clone(),
        job.resolve_or_init_file_name(),
        file_path.as_ref().clone(),
        total_size,
        checksums,
    )
}

async fn handle_existing_file(
    job: &Arc<Task>,
    file_path: &Arc<PathBuf>,
    probe: OriginMetadata,
) -> Result<bool, Error> {
    if !file_path.as_ref().exists() {
        reset_missing_file_state(job).await?;
        return Ok(false);
    }

    let metadata = fs::metadata(file_path.as_ref())
        .await
        .map_err(|e| Error::Io(format!("Failed to read file metadata: {}", e)))?;
    let current_size = metadata.len();

    match job.config.file_conflict_strategy {
        FileConflictStrategy::Overwrite => {
            fs::remove_file(file_path.as_ref())
                .await
                .map_err(|e| Error::Io(format!("Failed to remove existing file: {}", e)))?;
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

async fn verify_existing_file(job: &Arc<Task>, file_path: &Arc<PathBuf>) -> Result<bool, Error> {
    let mut checksums = job.checksums.lock().await;
    verify_file_checksums(checksums.as_mut_slice(), file_path.as_ref())
}

async fn can_resume_partial_download(job: &Arc<Task>, supports_range_requests: bool) -> bool {
    if !supports_range_requests {
        return false;
    }

    let workers = job.workers.read().await;
    !workers.is_empty()
}

async fn reset_missing_file_state(job: &Arc<Task>) -> Result<(), Error> {
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

async fn reset_resume_state(job: &Arc<Task>) -> Result<(), Error> {
    job.downloaded_size.store(0, Ordering::Relaxed);
    job.clear_task_workers().await?;
    job.purge_task_workers().await?;
    Ok(())
}
