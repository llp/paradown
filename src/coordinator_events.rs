use crate::coordinator_queue::{release_task_permit, remove_from_queue, spawn_next_task};
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::manager::DownloadManager;
use log::{error, warn};
use std::sync::Arc;
use tokio::sync::broadcast;

pub(crate) fn spawn_task_event_loop(manager: Arc<DownloadManager>) {
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

async fn handle_task_event(
    manager: &Arc<DownloadManager>,
    event: DownloadEvent,
) -> Result<(), DownloadError> {
    match event {
        DownloadEvent::Complete(task_id) => handle_terminal_task_event(manager, task_id).await,
        DownloadEvent::Cancel(task_id) => handle_terminal_task_event(manager, task_id).await,
        DownloadEvent::Error(task_id, _err) => handle_terminal_task_event(manager, task_id).await,
        DownloadEvent::Pause(task_id) => handle_terminal_task_event(manager, task_id).await,
        DownloadEvent::Preparing(task_id) => {
            manager.persist_task(task_id).await?;
            Ok(())
        }
        DownloadEvent::Progress { id, .. } => {
            manager.persist_task(id).await?;
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn handle_terminal_task_event(
    manager: &Arc<DownloadManager>,
    task_id: u32,
) -> Result<(), DownloadError> {
    manager.persist_task(task_id).await?;
    remove_from_queue(manager, task_id).await?;
    release_task_permit(manager, task_id).await;
    spawn_next_task(manager).await
}
