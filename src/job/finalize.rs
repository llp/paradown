use crate::Status;
use crate::error::Error;
use crate::events::Event;
use crate::job::Task;
use chrono::Utc;
use log::debug;
use std::path::Path;
use std::sync::Arc;

pub(crate) async fn finalize_download(job: &Arc<Task>) -> Result<(), Error> {
    let outcome = if let Some(path) = job.file_path.get() {
        match verify_checksums(job, path.as_ref()).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::Other("Checksum failed".into())),
            Err(err) => Err(err),
        }
    } else {
        Err(Error::Other(
            "No file path set for task, cannot verify checksum".into(),
        ))
    };

    finish_job(job, outcome).await
}

pub(crate) async fn finish_job(job: &Arc<Task>, outcome: Result<(), Error>) -> Result<(), Error> {
    match outcome {
        Ok(()) => {
            {
                let mut status = job.status.lock().await;
                *status = Status::Completed;
            }

            debug!("[Task {}] Download job completed", job.id);
            job.persist_task_checksums().await?;
            job.persist_task().await?;

            if let Some(manager) = job.manager.upgrade() {
                let _ = manager.task_event_tx.send(Event::Complete(job.id));
            }
        }
        Err(err) => {
            {
                let mut status = job.status.lock().await;
                *status = Status::Failed(err.clone());
            }

            debug!("[Task {}] Download job failed: {:?}", job.id, err);
            job.persist_task_checksums().await?;
            job.persist_task().await?;

            if let Some(manager) = job.manager.upgrade() {
                let _ = manager
                    .task_event_tx
                    .send(Event::Error(job.id, err.clone()));
            }
        }
    }

    Ok(())
}

pub(crate) async fn verify_checksums(job: &Arc<Task>, file_path: &Path) -> Result<bool, Error> {
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
