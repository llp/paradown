pub(crate) mod retry;
pub(crate) mod runtime;
pub(crate) mod transfer;

use crate::config::Config;
use crate::domain::{DownloadSpec, SourceDescriptor};
use crate::error::Error;
use crate::events::Event;
use crate::job::Task;
use crate::scheduler::planner::ExecutionLaneAssignment;
use crate::stats::Stats;
use crate::status::Status;
use chrono::{DateTime, Utc};
use log::debug;
use reqwest::Client;
use std::fmt;
use std::path::PathBuf;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::sync::Mutex;

pub struct Worker {
    pub task: Weak<Task>,
    pub config: Arc<Config>,
    pub client: Arc<Client>,
    pub id: u32,
    pub spec: DownloadSpec,
    source: std::sync::RwLock<SourceDescriptor>,
    lane: std::sync::RwLock<ExecutionLaneAssignment>,
    pub length_known: bool,
    pub start: u64,
    pub end: u64,
    pub downloaded_size: AtomicU64,
    pub total_size: AtomicU64,
    pub file_path: Arc<PathBuf>,
    pub paused: Arc<AtomicBool>,
    pub canceled: Arc<AtomicBool>,
    pub deleted: Arc<AtomicBool>,
    pub updated_at: Mutex<Option<DateTime<Utc>>>,
    pub status: Arc<tokio::sync::Mutex<Status>>,
    pub stats: Arc<Stats>,
    pub is_running: AtomicBool,
}

impl Worker {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: u32,
        config: Arc<Config>,
        task: Weak<Task>,
        client: Arc<Client>,
        spec: DownloadSpec,
        source: SourceDescriptor,
        lane: ExecutionLaneAssignment,
        downloaded_size: Option<u64>,
        file_path: Arc<PathBuf>,
        status: Option<Status>,
        stats: Arc<Stats>,
        updated_at: Option<DateTime<Utc>>,
    ) -> Self {
        debug!(
            "[Worker {}] Created for locator: {}, range: {}-{}",
            id, source.locator, lane.start, lane.end
        );

        let (paused, canceled, deleted) = match status {
            Some(Status::Paused) => (true, false, false),
            Some(Status::Canceled) => (false, true, false),
            Some(Status::Deleted) => (false, false, true),
            _ => (false, false, false),
        };
        let now = Utc::now();

        Self {
            config,
            task,
            client,
            id,
            spec,
            source: std::sync::RwLock::new(source),
            lane: std::sync::RwLock::new(lane.clone()),
            length_known: lane.length_known,
            start: lane.start,
            end: lane.end,
            file_path,
            downloaded_size: AtomicU64::new(downloaded_size.unwrap_or(0)),
            total_size: AtomicU64::new(lane.end.saturating_sub(lane.start).saturating_add(1)),
            paused: Arc::new(AtomicBool::new(paused)),
            canceled: Arc::new(AtomicBool::new(canceled)),
            deleted: Arc::new(AtomicBool::new(deleted)),
            updated_at: Mutex::new(Some(updated_at.unwrap_or(now))),
            status: Arc::new(tokio::sync::Mutex::new(status.unwrap_or(Status::Pending))),
            stats,
            is_running: AtomicBool::new(false),
        }
    }

    pub(crate) async fn set_status(&self, status: Status) {
        *self.status.lock().await = status;
    }

    pub(crate) fn current_source(&self) -> SourceDescriptor {
        self.source
            .read()
            .expect("worker source lock poisoned")
            .clone()
    }

    pub(crate) fn current_source_id(&self) -> String {
        self.current_source().id
    }

    pub(crate) fn lane_snapshot(&self) -> ExecutionLaneAssignment {
        self.lane.read().expect("worker lane lock poisoned").clone()
    }

    pub(crate) fn switch_source(&self, source: SourceDescriptor) {
        {
            let mut current = self.source.write().expect("worker source lock poisoned");
            *current = source.clone();
        }
        let mut lane = self.lane.write().expect("worker lane lock poisoned");
        lane.source_id = source.id;
    }

    pub(crate) fn emit_worker_event(&self, event: Event) {
        if let Some(task_arc) = self.task.upgrade() {
            let _ = task_arc.worker_event_tx.send(event);
        }
    }

    pub(crate) fn should_stop_gracefully(&self) -> bool {
        if self.deleted.load(Ordering::Relaxed) {
            debug!("[Worker {}] Download deleted", self.id);
            return true;
        }

        if self.canceled.load(Ordering::Relaxed) {
            debug!("[Worker {}] Download canceled", self.id);
            return true;
        }

        false
    }

    pub(crate) async fn wait_until_resumed(&self) -> bool {
        while self.paused.load(Ordering::Relaxed) {
            if self.should_stop_gracefully() {
                return false;
            }

            debug!("[Worker {}] Paused, waiting...", self.id);
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        true
    }

    pub async fn pause(&self) -> Result<(), Error> {
        self.paused.store(true, Ordering::Relaxed);

        let should_emit = {
            let mut status_guard = self.status.lock().await;
            match *status_guard {
                Status::Canceled | Status::Failed(_) | Status::Completed | Status::Deleted => {
                    debug!(
                        "[Worker {}] Paused ignored — current status = {:?}",
                        self.id, *status_guard
                    );
                    return Ok(());
                }
                Status::Paused => false,
                _ => {
                    *status_guard = Status::Paused;
                    true
                }
            }
        };

        debug!("[Worker {}] Paused", self.id);
        if should_emit {
            self.emit_worker_event(Event::Pause(self.id));
        }

        Ok(())
    }

    pub async fn resume(&self) -> Result<(), Error> {
        let should_start = {
            let mut status_guard = self.status.lock().await;
            match *status_guard {
                Status::Canceled | Status::Failed(_) | Status::Completed | Status::Deleted => {
                    debug!(
                        "[Worker {}] Resume ignored — current status = {:?}",
                        self.id, *status_guard
                    );
                    return Ok(());
                }
                _ => {}
            }

            debug!("[Worker {}] Resumed", self.id);
            self.paused.store(false, Ordering::Relaxed);

            if self.is_running.load(Ordering::Relaxed) {
                *status_guard = Status::Running;
                false
            } else {
                *status_guard = Status::Running;
                true
            }
        };

        if should_start {
            self.start().await
        } else {
            Ok(())
        }
    }

    pub async fn cancel(&self) -> Result<(), Error> {
        self.canceled.store(true, Ordering::Relaxed);
        self.set_status(Status::Canceled).await;
        debug!("[Worker {}] Canceled", self.id);
        self.emit_worker_event(Event::Cancel(self.id));
        Ok(())
    }

    pub async fn delete(&self) -> Result<(), Error> {
        self.deleted.store(true, Ordering::Relaxed);
        self.set_status(Status::Deleted).await;
        debug!("[Worker {}] Deleted", self.id);
        self.emit_worker_event(Event::Delete(self.id));
        Ok(())
    }
}

impl fmt::Debug for Worker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Worker")
            .field("id", &self.id)
            .field("spec", &self.spec)
            .field("source", &self.current_source())
            .field("lane", &self.lane_snapshot())
            .field("start", &self.start)
            .field("end", &self.end)
            .field("downloaded_size", &self.downloaded_size)
            .field("total_size", &self.total_size)
            .field("file_path", &self.file_path)
            .field("updated_at", &self.updated_at)
            .field("status", &self.status)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::Worker;
    use crate::config::Config;
    use crate::domain::{DownloadSpec, SourceSet};
    use crate::scheduler::planner::ExecutionLaneAssignment;
    use crate::stats::Stats;
    use crate::status::Status;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Weak};

    #[tokio::test]
    async fn resume_restarts_a_paused_worker_without_deadlocking() {
        let worker = Worker::new(
            1,
            Arc::new(Config::default()),
            Weak::new(),
            Arc::new(reqwest::Client::builder().no_proxy().build().unwrap()),
            DownloadSpec::parse("https://example.com/file").unwrap(),
            SourceSet::for_spec(
                &DownloadSpec::parse("https://example.com/file").unwrap(),
                None,
            )
            .primary()
            .unwrap()
            .clone(),
            ExecutionLaneAssignment {
                lane_id: 1,
                source_id: "https::https://example.com/file".into(),
                length_known: true,
                piece_start: 0,
                piece_end: 0,
                block_start: 0,
                block_end: 0,
                start: 0,
                end: 0,
            },
            Some(1),
            Arc::new(PathBuf::from("/tmp/paradown-unused-worker-test")),
            Some(Status::Paused),
            Arc::new(Stats::new()),
            None,
        );

        worker.resume().await.unwrap();

        assert!(!worker.paused.load(Ordering::Relaxed));
        assert!(!worker.is_running.load(Ordering::Relaxed));
        assert!(matches!(&*worker.status.lock().await, Status::Completed));
    }
}
