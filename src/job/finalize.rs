use crate::Status;
use crate::error::Error;
use crate::events::Event;
use crate::job::Task;
use crate::payload::verifier::verify_file_checksums as verify_payload_checksums;
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
            job.mark_all_pieces_completed().await;
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
    let verified = verify_payload_checksums(checksums.as_mut_slice(), file_path)?;
    debug!(
        "[Task {}] Checksum verification result for {:?}: {}",
        job.id, file_path, verified
    );
    Ok(verified)
}
