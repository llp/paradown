use crate::coordinator::{Manager, PendingAction};
use crate::error::Error;
use log::{debug, error, warn};
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;

pub(crate) async fn acquire_task_permit_or_queue(
    manager: &Arc<Manager>,
    task_id: u32,
    action: PendingAction,
) -> Result<Option<OwnedSemaphorePermit>, Error> {
    match manager.semaphore.clone().try_acquire_owned() {
        Ok(permit) => {
            debug!(
                "[Manager] Task {} acquired permit for {} (permits left = {})",
                task_id,
                action_label(action),
                manager.semaphore.available_permits()
            );
            Ok(Some(permit))
        }
        Err(_) => {
            enqueue_task_action(manager, task_id, action).await?;
            Ok(None)
        }
    }
}

pub(crate) async fn release_task_permit(manager: &Manager, task_id: u32) {
    if let Some(task) = manager.get_task(task_id) {
        let mut guard = task.permit.lock().await;
        if guard.is_some() {
            *guard = None;
            debug!("[Task {}] Released semaphore permit", task_id);
        }
    } else {
        warn!("[Manager] Task {} not found when releasing permit", task_id);
    }

    debug!(
        "[Manager] release task {} permit (permits left after release = {})",
        task_id,
        manager.semaphore.available_permits()
    );
}

pub(crate) async fn remove_from_queue(manager: &Manager, task_id: u32) -> Result<(), Error> {
    let mut queue = manager.pending_queue.lock().await;
    if let Some(pos) = queue.iter().position(|(id, _action)| *id == task_id) {
        queue.remove(pos);
        debug!(
            "[Manager] Removed task {} from pending queue (new len={})",
            task_id,
            queue.len()
        );
    }
    Ok(())
}

pub(crate) async fn clear_pending_queue(manager: &Manager) -> Result<(), Error> {
    let mut queue = manager.pending_queue.lock().await;
    queue.clear();
    Ok(())
}

pub(crate) async fn spawn_next_task(manager: &Arc<Manager>) -> Result<(), Error> {
    debug!("[Manager] spawn_next_task invoked");

    let next_opt = {
        let mut queue = manager.pending_queue.lock().await;
        let queue_len_before = queue.len();
        debug!("[Manager] Queue length before pop: {}", queue_len_before);
        queue.pop_front()
    };

    if let Some((task_id, action)) = next_opt {
        let manager_clone = Arc::clone(manager);
        match action {
            PendingAction::Start => {
                if let Err(e) = Box::pin(manager_clone.start_task(task_id)).await {
                    error!("[Manager] Failed to start queued task {}: {:?}", task_id, e);
                }
            }
            PendingAction::Resume => {
                if let Err(e) = Box::pin(manager_clone.resume_task(task_id)).await {
                    error!(
                        "[Manager] Failed to resume queued task {}: {:?}",
                        task_id, e
                    );
                }
            }
            PendingAction::Retry => {
                if let Err(e) = Box::pin(manager_clone.start_task(task_id)).await {
                    error!("[Manager] Failed to retry task {}: {:?}", task_id, e);
                }
            }
        }
    }

    let queue_len_after = {
        let queue = manager.pending_queue.lock().await;
        queue.len()
    };
    debug!(
        "[Manager] spawn_next_task completed. Queue length after task = {}",
        queue_len_after
    );
    Ok(())
}

async fn enqueue_task_action(
    manager: &Arc<Manager>,
    task_id: u32,
    action: PendingAction,
) -> Result<(), Error> {
    let mut queue = manager.pending_queue.lock().await;
    if !queue.iter().any(|(id, queued_action)| {
        *id == task_id && std::mem::discriminant(queued_action) == std::mem::discriminant(&action)
    }) {
        queue.push_back((task_id, action));
        debug!(
            "[Manager] Task {} queued for {} (queue len = {}, permits={})",
            task_id,
            action_label(action),
            queue.len(),
            manager.semaphore.available_permits()
        );
    } else {
        debug!(
            "[Manager] Task {} already queued for {} (queue len={})",
            task_id,
            action_label(action),
            queue.len()
        );
    }

    Ok(())
}

fn action_label(action: PendingAction) -> &'static str {
    match action {
        PendingAction::Start => "start",
        PendingAction::Resume => "resume",
        PendingAction::Retry => "retry",
    }
}
