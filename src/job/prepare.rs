use crate::config::FileConflictStrategy;
use crate::discovery::origin::{OriginMetadata, discover_origin};
use crate::domain::{HttpResourceIdentity, SessionManifest};
use crate::error::Error;
use crate::job::Task;
use crate::job::finalize::{finish_job, verify_checksums};
use crate::payload::verifier::verify_file_checksums;
use crate::scheduler::planner::suggested_http_piece_size;
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

    let previous_identity = job.http_resource_identity().await;
    let protocol_probe = discover_origin(job.client.as_ref(), &job.spec, job.http_request_options())
        .await?;
    let origin_changed = previous_identity.validator_changed(&protocol_probe.resource_identity);
    job.update_protocol_probe(
        protocol_probe.total_size,
        protocol_probe.supports_range_requests,
    );
    job.set_http_resource_identity(protocol_probe.resource_identity.clone())
        .await;
    let resolved_file_name = job.initialize_file_name(protocol_probe.suggested_file_name.clone());
    let file_path = job.get_or_init_file_path(download_dir)?;
    debug!(
        "[Task {}] Probe result: total_size={} supports_range_requests={} resolved_file_name={} resolved_url={:?} etag={:?} last_modified={:?}",
        job.id,
        protocol_probe.total_size,
        protocol_probe.supports_range_requests,
        resolved_file_name,
        protocol_probe.resource_identity.resolved_url,
        protocol_probe.resource_identity.entity_tag,
        protocol_probe.resource_identity.last_modified
    );
    debug!("[Task {}] Download file path: {:?}", job.id, file_path);

    let manifest = build_single_file_manifest(job, &file_path, &protocol_probe).await;
    let payload_store = job.install_manifest(manifest).await;

    if handle_existing_file(
        job,
        &file_path,
        &protocol_probe,
        &previous_identity,
        origin_changed,
    )
    .await?
    {
        return Ok(PreparationOutcome::Finished);
    }

    let _ = job.sync_piece_progress_from_workers().await?;
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
    probe: &OriginMetadata,
) -> SessionManifest {
    let checksums = job.checksums.lock().await.clone();
    let requested_workers = if probe.supports_range_requests {
        job.config.segments_per_task
    } else {
        1
    };
    let piece_size = suggested_http_piece_size(probe.total_size, requested_workers);

    SessionManifest::for_single_file_with_piece_size(
        job.spec.clone(),
        job.resolve_or_init_file_name(),
        file_path.as_ref().clone(),
        probe.total_size,
        piece_size,
        checksums,
    )
}

async fn handle_existing_file(
    job: &Arc<Task>,
    file_path: &Arc<PathBuf>,
    probe: &OriginMetadata,
    previous_identity: &HttpResourceIdentity,
    origin_changed: bool,
) -> Result<bool, Error> {
    if !file_path.as_ref().exists() {
        reset_missing_file_state(job).await?;
        return Ok(false);
    }

    let metadata = fs::metadata(file_path.as_ref())
        .await
        .map_err(|e| Error::Io(format!("Failed to read file metadata: {}", e)))?;
    let current_size = metadata.len();

    if origin_changed {
        info!(
            "[Task {}] Remote validator changed, discarding local file and restarting download",
            job.id
        );
        fs::remove_file(file_path.as_ref()).await?;
        reset_resume_state(job).await?;
        return Ok(false);
    }

    if current_size > probe.total_size {
        info!(
            "[Task {}] Local file size {} exceeds remote size {}, restarting download",
            job.id, current_size, probe.total_size
        );
        fs::remove_file(file_path.as_ref()).await?;
        reset_resume_state(job).await?;
        return Ok(false);
    }

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
            let checksum_ok =
                verify_existing_file(job, file_path, probe, previous_identity).await?;
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
                if can_resume_partial_download(job, probe, previous_identity).await {
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
                let checksum_ok =
                    verify_existing_file(job, file_path, probe, previous_identity).await?;
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
    job: &Arc<Task>,
    file_path: &Arc<PathBuf>,
    probe: &OriginMetadata,
    previous_identity: &HttpResourceIdentity,
) -> Result<bool, Error> {
    let mut checksums = job.checksums.lock().await;
    if !checksums.is_empty() {
        return verify_file_checksums(checksums.as_mut_slice(), file_path.as_ref());
    }

    let file_size = fs::metadata(file_path.as_ref()).await?.len();
    if file_size != probe.total_size {
        return Ok(false);
    }

    if !previous_identity.has_resume_validator() {
        debug!(
            "[Task {}] Existing file has no checksum and no stored remote validator, forcing redownload",
            job.id
        );
        return Ok(false);
    }

    if previous_identity.validator_changed(&probe.resource_identity) {
        debug!(
            "[Task {}] Existing file validator no longer matches remote target",
            job.id
        );
        return Ok(false);
    }

    Ok(true)
}

async fn can_resume_partial_download(
    job: &Arc<Task>,
    probe: &OriginMetadata,
    previous_identity: &HttpResourceIdentity,
) -> bool {
    if !probe.supports_range_requests {
        return false;
    }

    if !previous_identity.has_resume_validator() {
        return false;
    }

    if previous_identity.validator_changed(&probe.resource_identity) {
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
    job.reset_piece_progress().await;
    job.clear_task_workers().await?;
    job.purge_task_workers().await?;
    Ok(())
}
