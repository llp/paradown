use crate::DownloadStatus;
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::task::DownloadTask;
use chrono::Utc;
use log::debug;
use std::path::Path;
use std::sync::Arc;

pub(crate) async fn finalize_download(job: &Arc<DownloadTask>) -> Result<(), DownloadError> {
    let outcome = if let Some(path) = job.file_path.get() {
        match verify_checksums(job, path.as_ref()).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(DownloadError::Other("Checksum failed".into())),
            Err(err) => Err(err),
        }
    } else {
        Err(DownloadError::Other(
            "No file path set for task, cannot verify checksum".into(),
        ))
    };

    finish_job(job, outcome).await
}

pub(crate) async fn finish_job(
    job: &Arc<DownloadTask>,
    outcome: Result<(), DownloadError>,
) -> Result<(), DownloadError> {
    match outcome {
        Ok(()) => {
            {
                let mut status = job.status.lock().await;
                *status = DownloadStatus::Completed;
            }

            debug!("[Task {}] Download job completed", job.id);
            job.persist_task_checksums().await?;
            job.persist_task().await?;

            if let Some(manager) = job.manager.upgrade() {
                let _ = manager.task_event_tx.send(DownloadEvent::Complete(job.id));
            }
        }
        Err(err) => {
            {
                let mut status = job.status.lock().await;
                *status = DownloadStatus::Failed(err.clone());
            }

            debug!("[Task {}] Download job failed: {:?}", job.id, err);
            job.persist_task_checksums().await?;
            job.persist_task().await?;

            if let Some(manager) = job.manager.upgrade() {
                let _ = manager
                    .task_event_tx
                    .send(DownloadEvent::Error(job.id, err.clone()));
            }
        }
    }

    Ok(())
}

pub(crate) async fn verify_checksums(
    job: &Arc<DownloadTask>,
    file_path: &Path,
) -> Result<bool, DownloadError> {
    let mut checksums = job.checksums.lock().await;

    for checksum in checksums.iter_mut() {
        match checksum.verify(file_path) {
            Ok(true) => {
                checksum.verified = Some(true);
                checksum.verified_at = Some(Utc::now());
                debug!(
                    "[Task {}] Checksum {:?} passed for file {:?}",
                    job.id, checksum.algorithm, file_path
                );
            }
            Ok(false) => {
                checksum.verified = Some(false);
                checksum.verified_at = Some(Utc::now());
                debug!(
                    "[Task {}] Checksum {:?} FAILED for file {:?}",
                    job.id, checksum.algorithm, file_path
                );
                return Ok(false);
            }
            Err(err) => {
                checksum.verified = Some(false);
                checksum.verified_at = Some(Utc::now());
                debug!(
                    "[Task {}] Checksum {:?} ERROR for file {:?}: {:?}",
                    job.id, checksum.algorithm, file_path, err
                );
                return Err(err);
            }
        }
    }

    Ok(true)
}
