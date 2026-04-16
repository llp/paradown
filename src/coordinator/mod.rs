pub(crate) mod events;
pub(crate) mod queue;
pub(crate) mod registry;

use self::events::spawn_task_event_loop;
use self::queue::{acquire_task_permit_or_queue, clear_pending_queue, spawn_next_task};
use self::registry::{add_task_with_workers, restore_tasks};
use crate::config::Config;
use crate::domain::DownloadSpec;
use crate::download::{Session, SessionRequest};
use crate::error::Error;
use crate::events::Event;
use crate::job::Task;
use crate::rate_limiter::DownloadRateLimiter;
use crate::request::TaskRequest;
use crate::runtime::{HttpSessionState, build_http_client};
use crate::storage::Store;
use dashmap::DashMap;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use log::{debug, error, info};
use std::collections::VecDeque;
use std::num::NonZeroU64;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::sync::{Mutex, OnceCell, Semaphore, broadcast};

pub struct Manager {
    pub config: Arc<Config>,
    pub tasks: Arc<DashMap<u32, Arc<Task>>>,
    pub persistence: OnceCell<Arc<Store>>,
    pub pending_queue: Arc<Mutex<VecDeque<(u32, PendingAction)>>>,
    pub(crate) progress_persist_state: Arc<DashMap<u32, ProgressPersistState>>,
    pub task_event_tx: broadcast::Sender<Event>,
    pub http_client: Arc<reqwest::Client>,
    pub(crate) http_session_state: Option<Arc<HttpSessionState>>,
    pub(crate) rate_limiter: Arc<DownloadRateLimiter>,

    pub(crate) semaphore: Arc<Semaphore>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingAction {
    Start,
    Resume,
    Retry,
}

#[derive(Clone, Copy)]
pub(crate) struct ProgressPersistState {
    pub(crate) last_persisted_at: Instant,
    pub(crate) last_downloaded: u64,
}

impl Manager {
    pub fn new(config: Config) -> Result<Arc<Self>, Error> {
        let (task_event_tx, _) = broadcast::channel(100);
        let max_concurrent = config.concurrent_tasks;
        let built_http_client = build_http_client(&config)?;
        let http_client = Arc::new(built_http_client.client);
        let rate_limiter = Arc::new(DownloadRateLimiter::new(config.rate_limit_kbps));

        let manager = Arc::new(Self {
            config: Arc::new(config),
            tasks: Arc::new(DashMap::new()),
            persistence: OnceCell::new(),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            pending_queue: Arc::new(Mutex::new(VecDeque::new())),
            progress_persist_state: Arc::new(DashMap::new()),
            task_event_tx,
            http_client,
            http_session_state: built_http_client.session_state.map(Arc::new),
            rate_limiter,
        });

        Ok(manager)
    }

    pub async fn init(self: &Arc<Self>) -> Result<(), Error> {
        let persistence = Arc::new(Store::new(self.config.clone()).await?);
        self.persistence
            .set(persistence)
            .map_err(|_| Error::ConfigError("Persistence already initialized".into()))?;

        //
        let download_dir = &self.config.download_dir;
        if !Path::new(download_dir).exists() {
            fs::create_dir_all(download_dir)
                .await
                .map_err(|e| Error::Io(format!("创建下载目录失败: {}", e)))?;
        }

        restore_tasks(self).await?;

        spawn_task_event_loop(Arc::clone(self));

        Ok(())
    }

    pub(crate) async fn add_task(
        self: &Arc<Self>,
        task_request: TaskRequest,
    ) -> Result<u32, Error> {
        add_task_with_workers(self, task_request, None).await
    }

    pub async fn add_session(
        self: &Arc<Self>,
        session_request: SessionRequest,
    ) -> Result<u32, Error> {
        self.add_task(session_request.into_inner()).await
    }

    pub async fn add_download(self: &Arc<Self>, spec: DownloadSpec) -> Result<u32, Error> {
        self.add_task(TaskRequest::builder(spec).build()).await
    }

    pub async fn start_task(self: &Arc<Self>, task_id: u32) -> Result<u32, Error> {
        self.run_task_transition(
            task_id,
            PendingAction::Start,
            |task| Box::pin(async move { task.start().await }),
            "Starting",
        )
        .await
    }

    pub async fn pause_task(self: &Arc<Self>, task_id: u32) -> Result<u32, Error> {
        if let Some(task_ref) = self.tasks.get(&task_id) {
            let task = Arc::clone(task_ref.value());
            task.pause().await?;
            Ok(task_id)
        } else {
            Err(Error::TaskNotFound(task_id))
        }
    }

    pub async fn resume_task(self: &Arc<Self>, task_id: u32) -> Result<u32, Error> {
        self.run_task_transition(
            task_id,
            PendingAction::Resume,
            |task| Box::pin(async move { task.resume().await }),
            "Resuming",
        )
        .await
    }

    pub async fn cancel_task(self: &Arc<Self>, task_id: u32) -> Result<u32, Error> {
        if let Some(task_ref) = self.tasks.get(&task_id) {
            let task = Arc::clone(task_ref.value());
            task.cancel().await?;
            Ok(task_id)
        } else {
            Err(Error::TaskNotFound(task_id))
        }
    }

    pub async fn delete_task(self: &Arc<Self>, task_id: u32) -> Result<u32, Error> {
        let task = {
            if let Some(task_ref) = self.tasks.get(&task_id) {
                Arc::clone(task_ref.value())
            } else {
                error!("[Task {}] Not found when trying to delete", task_id);
                return Err(Error::TaskNotFound(task_id));
            }
        };

        if let Err(e) = task.delete().await {
            error!("[Task {}] Failed to delete task: {:?}", task_id, e);
            return Err(e);
        }
        self.tasks.remove(&task_id);
        info!(
            "[Task {}] Deleted successfully and removed from map",
            task_id
        );
        Ok(task_id)
    }

    pub(crate) async fn persist_task(self: &Arc<Self>, task_id: u32) -> Result<u32, Error> {
        // 1.获取任务
        let Some(task) = self.get_task(task_id) else {
            error!(
                "[Manager] Task {} not found while persisting state",
                task_id
            );
            return Err(Error::Other(format!("Task {} not found", task_id)));
        };

        // 2.获取持久化管理器
        let Some(persistence) = &task.persistence else {
            debug!(
                "[Manager {}] No persistence layer attached, skipping save",
                task_id
            );
            return Ok(task_id);
        };

        // 3.执行保存操作
        match persistence.save_task(&task).await {
            Ok(_) => {
                // debug!("[Manager {}] Task state persisted successfully", task_id);
                Ok(task_id)
            }
            Err(e) => {
                error!(
                    "[Manager {}] Failed to persist task state: {:?}",
                    task_id, e
                );
                Err(e)
            }
        }
    }

    pub async fn start_all(self: &Arc<Self>) -> Result<(), Error> {
        if self.tasks.is_empty() {
            return Err(Error::Other("No tasks to start".into()));
        }

        // 使用 FuturesUnordered 并发尝试启动每个任务（但实际启动受 semaphore 控制）
        let mut futures = FuturesUnordered::new();

        for entry in self.tasks.iter() {
            let task_id = *entry.key();
            let manager_clone = Arc::clone(self);
            futures.push(tokio::spawn(async move {
                if let Err(e) = manager_clone.start_task(task_id).await {
                    // 打印错误，同时不阻塞其他任务
                    error!("[Task {}] Failed to start: {:?}", task_id, e);
                }
            }));
        }

        // 等待所有尝试入队/启动的任务完成（不等待下载本身完成）
        while futures.next().await.is_some() {}

        Ok(())
    }

    pub async fn pause_all(self: &Arc<Self>) -> Result<(), Error> {
        if self.tasks.is_empty() {
            return Err(Error::Other("No tasks to pause".into()));
        }

        clear_pending_queue(self).await?;

        let futures: Vec<_> = self
            .tasks
            .iter()
            .map(|entry| {
                let task = Arc::clone(entry.value());
                tokio::spawn(async move {
                    if let Err(e) = task.pause().await {
                        error!("[Task {}] Failed to pause: {:?}", task.id, e);
                    }
                })
            })
            .collect();

        for f in futures {
            let _ = f.await;
        }
        Ok(())
    }

    pub async fn resume_all(self: &Arc<Self>) -> Result<(), Error> {
        if self.tasks.is_empty() {
            return Err(Error::Other("No tasks to resume".into()));
        }

        let mut futures = FuturesUnordered::new();

        for entry in self.tasks.iter() {
            let task_id = *entry.key();
            let manager_clone = Arc::clone(self);
            futures.push(tokio::spawn(async move {
                if let Err(e) = manager_clone.resume_task(task_id).await {
                    error!("[Task {}] Failed to resume: {:?}", task_id, e);
                    if let Err(e) = spawn_next_task(&manager_clone).await {
                        error!("[Manager] Failed to spawn next task after resume failure of task {}: {:?}", task_id, e);
                    }
                }
            }));
        }
        while futures.next().await.is_some() {}

        Ok(())
    }

    pub async fn cancel_all(self: &Arc<Self>) -> Result<(), Error> {
        if self.tasks.is_empty() {
            return Err(Error::Other("No tasks to cancel".into()));
        }

        clear_pending_queue(self).await?;

        for entry in self.tasks.iter() {
            let task = Arc::clone(entry.value());
            if let Err(e) = task.cancel().await {
                error!("[Task {}] Failed to cancel: {:?}", task.id, e);
            }
        }
        Ok(())
    }

    pub async fn delete_all(self: &Arc<Self>) -> Result<(), Error> {
        if self.tasks.is_empty() {
            return Err(Error::Other("No tasks to delete".into()));
        }

        clear_pending_queue(self).await?;

        for entry in self.tasks.iter() {
            let task = Arc::clone(entry.value());
            if let Err(e) = task.delete().await {
                error!("Failed to delete task {}: {:?}", task.id, e);
            }
        }
        self.tasks.clear();
        Ok(())
    }

    pub(crate) fn get_task(&self, id: u32) -> Option<Arc<Task>> {
        self.get_task_by_id(id)
    }

    pub fn get_session(&self, id: u32) -> Option<Session> {
        self.get_task(id).map(Session::from_task)
    }

    pub(crate) fn get_task_by_id(&self, id: u32) -> Option<Arc<Task>> {
        self.tasks.get(&id).map(|v| Arc::clone(&v))
    }

    pub fn get_session_by_id(&self, id: u32) -> Option<Session> {
        self.get_task_by_id(id).map(Session::from_task)
    }

    pub(crate) fn get_task_by_locator(&self, locator: &str) -> Option<Arc<Task>> {
        self.tasks
            .iter()
            .find(|entry| entry.value().spec.locator() == locator)
            .map(|entry| Arc::clone(entry.value()))
    }

    pub fn get_session_by_locator(&self, locator: &str) -> Option<Session> {
        self.get_task_by_locator(locator).map(Session::from_task)
    }

    pub(crate) fn get_all_tasks(&self) -> Vec<Arc<Task>> {
        self.tasks
            .iter()
            .map(|entry| Arc::clone(entry.value()))
            .collect()
    }

    pub fn get_all_sessions(&self) -> Vec<Session> {
        self.get_all_tasks()
            .into_iter()
            .map(Session::from_task)
            .collect()
    }

    //----------------------------------------------------------------------------------------------
    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.task_event_tx.subscribe()
    }

    pub(crate) fn should_persist_progress(&self, task_id: u32, downloaded: u64) -> bool {
        let threshold = self.config.progress_throttle.threshold_bytes;
        let interval = Duration::from_millis(self.config.progress_throttle.interval_ms);
        let now = Instant::now();

        match self.progress_persist_state.get_mut(&task_id) {
            Some(mut state) => {
                let bytes_due = downloaded.saturating_sub(state.last_downloaded) >= threshold;
                let time_due = now.duration_since(state.last_persisted_at) >= interval;
                if bytes_due || time_due {
                    state.last_downloaded = downloaded;
                    state.last_persisted_at = now;
                    true
                } else {
                    false
                }
            }
            None => {
                self.progress_persist_state.insert(
                    task_id,
                    ProgressPersistState {
                        last_persisted_at: now,
                        last_downloaded: downloaded,
                    },
                );
                true
            }
        }
    }

    pub(crate) fn clear_progress_persist_state(&self, task_id: u32) {
        self.progress_persist_state.remove(&task_id);
    }

    pub async fn set_rate_limit_kbps(&self, limit_kbps: Option<NonZeroU64>) {
        self.rate_limiter.set_limit_kbps(limit_kbps).await;
    }

    pub fn current_rate_limit_kbps(&self) -> Option<u64> {
        self.rate_limiter.current_limit_kbps()
    }

    pub(crate) fn persist_http_session_state(&self) -> Result<(), Error> {
        if let Some(session_state) = &self.http_session_state {
            session_state.persist()?;
        }
        Ok(())
    }

    pub async fn wait_for_all_tasks(self: &Arc<Self>) -> Result<(), Error> {
        let mut rx = self.subscribe_events();

        loop {
            if self.tasks.is_empty() {
                self.persist_http_session_state()?;
                return Ok(());
            }

            let mut all_terminal = true;
            for task in self.get_all_tasks() {
                let status = task.status.lock().await.clone();
                if !status.is_terminal() {
                    all_terminal = false;
                    break;
                }
            }

            if all_terminal {
                self.persist_http_session_state()?;
                return Ok(());
            }

            match rx.recv().await {
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    }

    #[allow(clippy::needless_lifetimes)]
    async fn run_task_transition<'a, F>(
        self: &'a Arc<Self>,
        task_id: u32,
        action: PendingAction,
        operation: F,
        action_label: &str,
    ) -> Result<u32, Error>
    where
        F: FnOnce(
            Arc<Task>,
        )
            -> Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>>,
    {
        let Some(permit) = acquire_task_permit_or_queue(self, task_id, action).await? else {
            return Ok(task_id);
        };

        let task_ref = self
            .tasks
            .get(&task_id)
            .ok_or(Error::TaskNotFound(task_id))?;
        let task = Arc::clone(task_ref.value());

        debug!(
            "[Manager] {} task {} (permits left after acquire = {})",
            action_label,
            task_id,
            self.semaphore.available_permits()
        );

        let res = operation(Arc::clone(&task)).await;
        match res {
            Ok(()) => {
                debug!("[Manager] Task {} {} successfully", task_id, action_label);
                let mut guard = task.permit.lock().await;
                *guard = Some(permit);
                Ok(task_id)
            }
            Err(e) => {
                error!(
                    "[Manager] Task {} {} failed: {:?}",
                    task_id, action_label, e
                );
                drop(permit);
                let mut guard = task.permit.lock().await;
                *guard = None;

                if let Err(spawn_err) = spawn_next_task(self).await {
                    error!(
                        "Failed to spawn next task after {} task failed({}): {:?}",
                        action_label.to_lowercase(),
                        task_id,
                        spawn_err
                    );
                }
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Manager;
    use crate::Config;

    #[test]
    fn throttles_progress_persistence_until_threshold_or_interval() {
        let mut config = Config::default();
        config.progress_throttle.interval_ms = 60_000;
        config.progress_throttle.threshold_bytes = 1024;
        let manager = Manager::new(config).unwrap();

        assert!(manager.should_persist_progress(1, 128));
        assert!(!manager.should_persist_progress(1, 256));
        assert!(manager.should_persist_progress(1, 2048));
    }
}
