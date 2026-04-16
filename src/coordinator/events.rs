use crate::coordinator::Manager;
use crate::coordinator::queue::{release_task_permit, remove_from_queue, spawn_next_task};
use crate::diagnostics::write_failure_diagnostic;
use crate::error::Error;
use crate::events::Event;
use log::{error, warn};
use std::sync::Arc;
use tokio::sync::broadcast;

pub(crate) fn spawn_task_event_loop(manager: Arc<Manager>) {
    let mut rx = manager.task_event_tx.subscribe();

    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(err) = handle_task_event(&manager, event).await {
                        error!("[Manager] Failed to handle task event: {:?}", err);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        "[Manager] Task event consumer lagged and skipped {} events",
                        skipped
                    );
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

async fn handle_task_event(manager: &Arc<Manager>, event: Event) -> Result<(), Error> {
    match event {
        Event::Complete(task_id) => handle_terminal_task_event(manager, task_id).await,
        Event::Cancel(task_id) => handle_terminal_task_event(manager, task_id).await,
        Event::Delete(task_id) => handle_deleted_task_event(manager, task_id).await,
        Event::Error(task_id, err) => {
            if let Some(task) = manager.get_task_by_id(task_id)
                && let Err(diag_err) = write_failure_diagnostic(task.as_ref(), &err).await
            {
                warn!(
                    "[Manager] Failed to write diagnostic for task {}: {}",
                    task_id, diag_err
                );
            }
            handle_terminal_task_event(manager, task_id).await
        }
        Event::Pause(task_id) => handle_terminal_task_event(manager, task_id).await,
        Event::Preparing(task_id) => {
            manager.persist_task(task_id).await?;
            Ok(())
        }
        Event::Progress { id, downloaded, .. } => {
            if manager.should_persist_progress(id, downloaded) {
                manager.persist_task(id).await?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn handle_terminal_task_event(manager: &Arc<Manager>, task_id: u32) -> Result<(), Error> {
    manager.persist_task(task_id).await?;
    manager.persist_http_session_state()?;
    manager.clear_progress_persist_state(task_id);
    remove_from_queue(manager, task_id).await?;
    release_task_permit(manager, task_id).await;
    spawn_next_task(manager).await
}

async fn handle_deleted_task_event(manager: &Arc<Manager>, task_id: u32) -> Result<(), Error> {
    manager.persist_http_session_state()?;
    manager.clear_progress_persist_state(task_id);
    remove_from_queue(manager, task_id).await?;
    spawn_next_task(manager).await
}
