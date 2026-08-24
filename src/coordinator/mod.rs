//! 下载任务协调器。
//!
//! 这个模块的核心类型是 [`Manager`]。它不负责真正传输文件，而是负责在多个异步任务之间
//! 共享配置、登记任务、限制同时运行的任务数、排队、接收事件以及触发持久化。
//!
//! 从 Rust 语法角度，这个文件集中展示了：
//! - 模块可见性：`pub(crate) mod`、`self::`、`crate::`。
//! - 共享所有权：`Arc<T>` 和 `Weak<T>`（后者在创建 `Task` 时使用）。
//! - 并发容器：`DashMap`、异步 `Mutex`、`Semaphore`、`broadcast`。
//! - 一次性初始化：`OnceCell<T>`。
//! - 异步并发：`async fn`、`.await`、`tokio::spawn`、`FuturesUnordered`。
//! - 高阶函数：闭包、`FnOnce`、`dyn Future`、`Pin<Box<...>>` 和生命周期 `'a`。
//!
//! 子模块职责：
//! - `registry`：创建任务、恢复任务，并把任务登记到 `Manager.tasks`。
//! - `queue`：获取 semaphore permit；没有名额时把操作放进等待队列。
//! - `events`：消费任务事件；任务结束后持久化、释放 permit、启动下一个任务。

// `pub(crate)` 表示模块在当前 crate 内公开，但不会成为库对外 API。
// 外部使用者不能通过 `paradown::coordinator::queue` 访问这些实现细节。
pub(crate) mod events;
pub(crate) mod queue;
pub(crate) mod registry;

// `self::` 从当前模块开始解析路径。这里从上面声明的三个子模块引入函数。
use self::events::spawn_task_event_loop;
use self::queue::{acquire_task_permit_or_queue, clear_pending_queue, spawn_next_task};
use self::registry::{add_task_with_workers, restore_tasks};
// `crate::` 从当前 crate 根开始解析路径，不受当前模块嵌套层级影响。
use crate::config::Config;
use crate::domain::DownloadSpec;
use crate::download::{Session, SessionRequest};
use crate::error::Error;
use crate::events::Event;
use crate::job::Task;
use crate::p2p::{TorrentEngine, default_libtorrent_engine};
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

const TASK_EVENT_CHANNEL_CAPACITY: usize = 4096;

/// 全局下载任务协调器。
///
/// `Manager` 会被 CLI、事件循环、任务和 worker 同时使用，因此构造函数返回 `Arc<Manager>`。
/// 字段中再次出现的 `Arc<T>` 表示这些资源还会独立共享给其他对象。
///
/// 这里没有给 `Manager` 实现 `Clone`。需要共享它时克隆的是 `Arc<Manager>`，
/// 克隆 `Arc` 只增加原子引用计数，不会复制整个 `Manager`。
pub struct Manager {
    /// `Arc<Config>`：多个任务共享同一份只读配置。
    pub config: Arc<Config>,
    /// `DashMap<K, V>` 是支持并发访问的 map。
    ///
    /// 外层 `Arc` 允许多个对象共享 map；值 `Arc<Task>` 允许 map、worker 和 session
    /// 同时拥有同一个任务。两层 `Arc` 分别共享“容器”和“容器中的任务”。
    pub tasks: Arc<DashMap<u32, Arc<Task>>>,
    /// `OnceCell<Arc<Store>>` 表示持久化存储可以稍后初始化，但最多成功设置一次。
    ///
    /// `Manager::new` 是同步函数，而 `Store::new` 是异步函数，因此存储不能在构造器中完成；
    /// 它会在 `init().await` 中写入 OnceCell。
    pub persistence: OnceCell<Arc<Store>>,
    /// 等待队列需要跨异步任务共享并修改，所以组合为 `Arc<Mutex<VecDeque<_>>>`。
    ///
    /// `VecDeque` 负责先进先出；Tokio `Mutex` 负责异步互斥；`Arc` 负责共享所有权。
    /// `(u32, PendingAction)` 是元组，分别保存任务 id 和待执行动作。
    pub pending_queue: Arc<Mutex<VecDeque<(u32, PendingAction)>>>,
    /// 每个任务最近一次持久化进度，用并发 map 避免全局单锁。
    pub(crate) progress_persist_state: Arc<DashMap<u32, ProgressPersistState>>,
    /// broadcast channel 的发送端。一个事件可被多个订阅者同时收到。
    pub task_event_tx: broadcast::Sender<Event>,
    /// reqwest client 内部本来就适合复用；外层 Arc 让 Task/Worker 共享同一个 client。
    pub http_client: Arc<reqwest::Client>,
    /// `Option<Arc<T>>` 表示 HTTP 会话状态可能不存在；存在时可被共享。
    pub(crate) http_session_state: Option<Arc<HttpSessionState>>,
    pub(crate) rate_limiter: Arc<DownloadRateLimiter>,
    /// `dyn TorrentEngine` 是 trait object：运行时可以放入任意实现 `TorrentEngine` 的类型。
    ///
    /// `Arc<dyn Trait>` 同时提供共享所有权和动态分发。调用 `capabilities()` 等方法时，
    /// 会通过虚表找到实际引擎类型的实现。
    pub(crate) torrent_engine: Arc<dyn TorrentEngine>,

    /// Semaphore 控制最多有多少任务同时运行。
    /// 每个运行任务持有一个 permit；permit 被 drop 时，名额自动归还。
    pub(crate) semaphore: Arc<Semaphore>,
}

/// 等待队列中记录的任务动作。
///
/// `Copy` 表示这个小 enum 可以按位复制，赋值或传参后原值仍可继续使用。
/// `Eq` / `PartialEq` 允许比较；`Debug` 支持 `{:?}`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingAction {
    Start,
    Resume,
    Retry,
}

/// 进度持久化节流状态。
///
/// 两个字段都是可复制的小值，因此整个结构体可以 derive `Copy`。
#[derive(Clone, Copy)]
pub(crate) struct ProgressPersistState {
    pub(crate) last_persisted_at: Instant,
    pub(crate) last_downloaded: u64,
}

impl Manager {
    /// 使用默认 torrent engine 创建 Manager。
    ///
    /// 这是关联函数，因为参数中没有 `self`。返回 `Arc<Self>` 而不是 `Self`，
    /// 表明 Manager 从创建开始就按共享对象使用。
    pub fn new(config: Config) -> Result<Arc<Self>, Error> {
        let torrent_engine = default_libtorrent_engine(config.p2p.libtorrent.clone());
        Self::new_with_torrent_engine(config, torrent_engine)
    }

    /// 注入一个 torrent engine 创建 Manager。
    ///
    /// 参数 `Arc<dyn TorrentEngine>` 接受具体类型被擦除后的 trait object，常用于替换实现。
    /// `Self::new_with_torrent_engine` 中的 `Self` 等价于 `Manager`。
    pub fn new_with_torrent_engine(
        config: Config,
        torrent_engine: Arc<dyn TorrentEngine>,
    ) -> Result<Arc<Self>, Error> {
        // `?`：验证失败时把 Err 直接返回；成功时取出 `()` 并继续。
        config.validate()?;
        // `_` 通配绑定表示故意忽略初始 receiver；这里只保留 Sender。
        let (task_event_tx, _) = broadcast::channel(TASK_EVENT_CHANNEL_CAPACITY);
        let max_concurrent = config.concurrent_tasks;
        let built_http_client = build_http_client(&config)?;
        let http_client = Arc::new(built_http_client.client);
        let rate_limiter = Arc::new(DownloadRateLimiter::new(config.rate_limit_kib_per_sec));

        // 结构体表达式中的字段简写：`task_event_tx,` 等价于
        // `task_event_tx: task_event_tx,`。
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
            torrent_engine,
        });

        Ok(manager)
    }

    /// 完成需要异步执行的初始化。
    ///
    /// `self: &Arc<Self>` 是显式方法接收器：方法借用的不是裸 `Manager`，而是 `Arc<Manager>`。
    /// 这样方法内部可以直接 `Arc::clone(self)`，把 Manager 交给 `'static` 异步任务。
    ///
    /// 普通 `&self` 只能得到 `&Manager`，无法直接产生新的 `Arc<Manager>` 所有者。
    pub async fn init(self: &Arc<Self>) -> Result<(), Error> {
        // `self.config.clone()` 克隆 Arc，不会复制 Config。
        let persistence = Arc::new(Store::new(self.config.clone()).await?);
        // OnceCell::set 成功后值永久存在；再次 set 会返回 Err(原值)。
        // `map_err` 把该错误映射为项目自己的 Error。
        self.persistence
            .set(persistence)
            .map_err(|_| Error::ConfigError("Persistence already initialized".into()))?;

        let download_dir = &self.config.download_dir;
        if !Path::new(download_dir).exists() {
            fs::create_dir_all(download_dir)
                .await
                .map_err(|e| Error::Io(format!("创建下载目录失败: {}", e)))?;
        }

        restore_tasks(self).await?;

        // 克隆 Arc 后，后台事件循环独立拥有 Manager；init 返回后它仍然有效。
        spawn_task_event_loop(Arc::clone(self));

        Ok(())
    }

    pub fn torrent_engine_capabilities(&self) -> crate::p2p::TorrentEngineCapabilities {
        self.torrent_engine.capabilities()
    }

    pub(crate) async fn add_task(
        self: &Arc<Self>,
        task_request: TaskRequest,
    ) -> Result<u32, Error> {
        // `task_request` 按值传入，因此它的所有权移动到 registry 函数。
        add_task_with_workers(self, task_request, None).await
    }

    /// 添加公共 Session 请求。
    ///
    /// `into_inner()` 消费 `SessionRequest`，取出其中拥有的 `TaskRequest`，避免 clone。
    pub async fn add_session(
        self: &Arc<Self>,
        session_request: SessionRequest,
    ) -> Result<u32, Error> {
        self.add_task(session_request.into_inner()).await
    }

    /// 用下载规格构造最小 TaskRequest 并登记任务。
    ///
    /// builder 链中的 `build()` 消费 builder，最终 `TaskRequest` 再移动给 `add_task`。
    pub async fn add_download(self: &Arc<Self>, spec: DownloadSpec) -> Result<u32, Error> {
        self.add_task(TaskRequest::builder(spec).build()).await
    }

    /// 启动任务。
    ///
    /// 这里把具体操作作为闭包传给 `run_task_transition`。`async move` 把闭包参数 `task`
    /// 移动进 Future；`Box::pin` 把 Future 放到堆上并固定地址，统一成 trait object 返回类型。
    pub async fn start_task(self: &Arc<Self>, task_id: u32) -> Result<u32, Error> {
        self.run_task_transition(
            task_id,
            PendingAction::Start,
            |task| Box::pin(async move { task.start().await }),
            "Starting",
        )
        .await
    }

    /// 暂停单个任务。
    ///
    /// `DashMap::get` 返回一个 guard，不是裸 `Arc<Task>`。在 `.await` 前先克隆内部 Arc，
    /// 可以让 DashMap guard 尽快释放，避免跨 await 持有分片锁。
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
        // 独立作用域让 `task_ref` guard 在代码块结束时释放。
        // 得到的 `task` 是 Arc 克隆，可以安全地跨后面的 `.await` 使用。
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
        // `let ... else` 要求模式匹配成功；不成功就执行必须发散的 else（这里是 return）。
        let Some(task) = self.get_task(task_id) else {
            error!(
                "[Manager] Task {} not found while persisting state",
                task_id
            );
            return Err(Error::Other(format!("Task {} not found", task_id)));
        };

        // 这里匹配的是 `&Option<Arc<Store>>`，成功后 `persistence` 是借用，
        // 不会把 Arc 从 task 中移动出来。
        let Some(persistence) = &task.persistence else {
            debug!(
                "[Manager {}] No persistence layer attached, skipping save",
                task_id
            );
            return Ok(task_id);
        };

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

    /// 并发尝试启动所有任务。
    ///
    /// `FuturesUnordered` 是一个 Future 集合：其中任意 Future 完成后就能被取出，
    /// 不要求按插入顺序完成。这里等待的是“每个启动请求处理完”，不是下载完成。
    pub async fn start_all(self: &Arc<Self>) -> Result<(), Error> {
        if self.tasks.is_empty() {
            return Err(Error::Other("No tasks to start".into()));
        }

        let mut futures = FuturesUnordered::new();

        // `self.tasks.iter()` 返回 DashMap 的 guard entry。
        // `*entry.key()` 解引用 `&u32`；u32 实现 Copy，所以得到独立 task_id。
        for entry in self.tasks.iter() {
            let task_id = *entry.key();
            // 每个 spawn 的任务都需要拥有 Manager，因此为每个任务增加一个 Arc 所有者。
            let manager_clone = Arc::clone(self);
            // `tokio::spawn` 要求 Future 通常是 `Send + 'static`。
            // `async move` 把 task_id 和 manager_clone 移动进 Future，使它不借用当前栈帧。
            futures.push(tokio::spawn(async move {
                if let Err(e) = manager_clone.start_task(task_id).await {
                    error!("[Task {}] Failed to start: {:?}", task_id, e);
                }
            }));
        }

        // `StreamExt::next()` 把 FuturesUnordered 当作 Stream 消费。
        // `.is_some()` 为 true 表示还有一个已完成结果；None 表示集合为空。
        while futures.next().await.is_some() {}

        Ok(())
    }

    /// 并发暂停所有任务。
    ///
    /// 这里展示了 iterator + closure + collect 构造 `Vec<JoinHandle<()>>`。
    pub async fn pause_all(self: &Arc<Self>) -> Result<(), Error> {
        if self.tasks.is_empty() {
            return Err(Error::Other("No tasks to pause".into()));
        }

        clear_pending_queue(self).await?;

        // `Vec<_>` 中的 `_` 让编译器从 `.collect()` 的元素推断具体类型为 JoinHandle<()>。
        let futures: Vec<_> = self
            .tasks
            .iter()
            .map(|entry| {
                // 在闭包内克隆 Arc，使 DashMap entry guard 不会被捕获进异步任务。
                let task = Arc::clone(entry.value());
                tokio::spawn(async move {
                    if let Err(e) = task.pause().await {
                        error!("[Task {}] Failed to pause: {:?}", task.id, e);
                    }
                })
            })
            .collect();

        // `for f in futures` 会消费 Vec，逐个移动出 JoinHandle。
        for f in futures {
            // `let _ =` 明确忽略 JoinError；暂停失败已在任务内部记录日志。
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

    /// 取消所有任务。
    ///
    /// `get_all_tasks()` 返回独立的 `Vec<Arc<Task>>`，因此循环中的 `.await`
    /// 不会在等待期间持有 DashMap 的 entry guard。
    pub async fn cancel_all(self: &Arc<Self>) -> Result<(), Error> {
        if self.tasks.is_empty() {
            return Err(Error::Other("No tasks to cancel".into()));
        }

        clear_pending_queue(self).await?;

        for task in self.get_all_tasks() {
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

        for task in self.get_all_tasks() {
            if let Err(e) = task.delete().await {
                error!("Failed to delete task {}: {:?}", task.id, e);
            }
        }
        self.tasks.clear();
        Ok(())
    }

    /// 按 id 获取任务的共享所有权。
    ///
    /// 返回 `Option<Arc<Task>>` 而不是 DashMap guard，调用方可安全地跨 `.await` 保存结果。
    pub(crate) fn get_task(&self, id: u32) -> Option<Arc<Task>> {
        self.get_task_by_id(id)
    }

    pub fn get_session(&self, id: u32) -> Option<Session> {
        self.get_task(id).map(Session::from_task)
    }

    pub(crate) fn get_task_by_id(&self, id: u32) -> Option<Arc<Task>> {
        // `map` 只在 get 返回 Some 时执行闭包。
        // `Arc::clone(&v)` 依赖自动解引用：v 是 DashMap guard，可解引用到 Arc<Task>。
        self.tasks.get(&id).map(|v| Arc::clone(&v))
    }

    pub fn get_session_by_id(&self, id: u32) -> Option<Session> {
        self.get_task_by_id(id).map(Session::from_task)
    }

    /// 按 locator 线性查找任务。
    ///
    /// `.find(...)` 返回第一个匹配 entry；随后 `.map(...)` 克隆其内部 Arc。
    /// 闭包只借用 locator，不取得传入字符串的所有权。
    pub(crate) fn get_task_by_locator(&self, locator: &str) -> Option<Arc<Task>> {
        self.tasks
            .iter()
            .find(|entry| entry.value().spec.locator() == locator)
            .map(|entry| Arc::clone(entry.value()))
    }

    pub fn get_session_by_locator(&self, locator: &str) -> Option<Session> {
        self.get_task_by_locator(locator).map(Session::from_task)
    }

    /// 收集所有任务的 Arc 克隆。
    ///
    /// `.collect()` 的目标类型由返回签名 `Vec<Arc<Task>>` 推断。
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

    /// 订阅任务事件。
    ///
    /// 每次 `subscribe()` 都创建一个新的 receiver；后续广播的事件会发送给所有活跃订阅者。
    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.task_event_tx.subscribe()
    }

    /// 判断本次进度是否应该持久化。
    ///
    /// `DashMap::get_mut` 返回一个可变 guard。通过 guard 修改字段时，DashMap 对应分片被锁定；
    /// guard 在 match 分支结束时自动 drop。
    pub(crate) fn should_persist_progress(&self, task_id: u32, downloaded: u64) -> bool {
        let threshold = self.config.progress_throttle.threshold_bytes;
        let interval = Duration::from_millis(self.config.progress_throttle.interval_ms);
        let now = Instant::now();

        match self.progress_persist_state.get_mut(&task_id) {
            Some(mut state) => {
                // `saturating_sub` 在 downloaded 小于旧值时返回 0，不会发生无符号整数下溢。
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

    pub async fn set_rate_limit_kib_per_sec(&self, limit_kib_per_sec: Option<NonZeroU64>) {
        self.rate_limiter
            .set_limit_kib_per_sec(limit_kib_per_sec)
            .await;
    }

    pub fn current_rate_limit_kib_per_sec(&self) -> Option<u64> {
        self.rate_limiter.current_limit_kib_per_sec()
    }

    pub(crate) fn persist_http_session_state(&self) -> Result<(), Error> {
        if let Some(session_state) = &self.http_session_state {
            session_state.persist()?;
        }
        Ok(())
    }

    /// 等待所有任务进入终态。
    ///
    /// 该函数不是忙循环：如果任务还没全部结束，就在 `rx.recv().await` 挂起等待下一个事件。
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
                // broadcast receiver 太慢时会收到 Lagged，并报告跳过数量。
                // 这里忽略该错误并重新检查全部任务状态，因为状态本身是最终事实来源。
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            }
        }
    }

    /// 执行一个需要占用并发名额的异步任务操作。
    ///
    /// 先不要看完整泛型签名。这个函数在概念上可以先理解成：
    ///
    /// ```text
    /// run_task_transition(
    ///     任务 id,
    ///     要排队的动作类型,
    ///     一个“拿到 Task 后执行异步操作”的函数,
    ///     日志文字,
    /// )
    /// ```
    ///
    /// `start_task` 中的实际调用是：
    ///
    /// ```ignore
    /// self.run_task_transition(
    ///     task_id,
    ///     PendingAction::Start,
    ///     |task| Box::pin(async move { task.start().await }),
    ///     "Starting",
    /// )
    /// .await
    /// ```
    ///
    /// 所以 `operation` 实际上就是这一段闭包：
    ///
    /// ```ignore
    /// |task| Box::pin(async move { task.start().await })
    /// ```
    ///
    /// 可以按从内向外的顺序阅读这个闭包：
    ///
    /// ```text
    /// task.start().await
    ///     先异步启动任务，最终得到 Result<(), Error>
    ///
    /// async move { ... }
    ///     把 task 移进异步块，创建一个 Future
    ///
    /// Box::pin(...)
    ///     把这个 Future 放到堆上并固定地址
    ///
    /// |task| ...
    ///     整体是一个接收 Arc<Task>、返回 Future 的闭包
    /// ```
    ///
    /// 完整签名之所以很长，是因为 Rust 要精确描述这个闭包的输入类型和返回类型。
    /// 它并不是额外执行了很多事情，只是在类型层面写清楚“异步函数参数”的形状。
    ///
    /// # 第一部分：`<'a, F>`
    ///
    /// ```ignore
    /// async fn run_task_transition<'a, F>(...)
    /// ```
    ///
    /// 尖括号中声明了两个泛型参数：
    ///
    /// - `'a` 是生命周期参数。前面的 `'` 表示它是生命周期，不是普通类型。
    /// - `F` 是类型参数。调用者传入的闭包有一个编译器生成的匿名类型，这个类型用 `F` 代表。
    ///
    /// 每个闭包都有自己独一无二、无法手写名字的具体类型，所以函数不能把参数写成某个普通结构体类型。
    /// 使用泛型 `F` 后，编译器会在调用处把 `F` 替换成那个闭包的真实匿名类型。
    ///
    /// # 第二部分：`self: &'a Arc<Self>`
    ///
    /// 普通方法常写 `&self`，这里使用的是显式 self 接收器：
    ///
    /// ```ignore
    /// self: &'a Arc<Self>
    /// ```
    ///
    /// 分解如下：
    ///
    /// - `Self` 在 `impl Manager` 中等价于 `Manager`。
    /// - `Arc<Self>` 等价于 `Arc<Manager>`。
    /// - `&Arc<Self>` 表示借用一个 Arc，不取得这个 Arc 的所有权。
    /// - `&'a Arc<Self>` 表示这个借用在生命周期 `'a` 内有效。
    ///
    /// 因此它可以近似读作：
    ///
    /// ```ignore
    /// self: &'a Arc<Manager>
    /// ```
    ///
    /// 这里让方法接收 `&Arc<Manager>`，是为了能在内部调用 `Arc::clone(self)`，
    /// 为异步任务创建新的共享所有者。
    ///
    /// # 第三部分：普通参数
    ///
    /// ```ignore
    /// task_id: u32,
    /// action: PendingAction,
    /// operation: F,
    /// action_label: &str,
    /// ```
    ///
    /// - `task_id` 是可复制的整数。
    /// - `action` 是可复制的小 enum，用于没有 permit 时排队。
    /// - `operation` 是调用者传入的闭包，具体类型就是泛型 `F`。
    /// - `action_label` 是借用的字符串切片，只用于日志。
    ///
    /// # 第四部分：函数本身的返回值
    ///
    /// ```ignore
    /// -> Result<u32, Error>
    /// ```
    ///
    /// 因为这是 `async fn`，源码中写的返回类型是 Future 完成之后的输出类型。
    /// 调用 `run_task_transition(...)` 先得到一个 Future；调用 `.await` 后才得到：
    ///
    /// ```ignore
    /// Result<u32, Error>
    /// ```
    ///
    /// # 第五部分：`where F: FnOnce(...)`
    ///
    /// `where` 子句不是执行代码，而是给泛型 `F` 增加类型约束：
    ///
    /// ```ignore
    /// F: FnOnce(Arc<Task>) -> 某个返回类型
    /// ```
    ///
    /// 它表示 `F` 必须像函数一样被调用：
    ///
    /// - 输入一个 `Arc<Task>`。
    /// - 输出一个可等待的 Future。
    ///
    /// `FnOnce` 是 Rust 的闭包调用 trait。`Once` 表示只保证能调用一次。
    /// 原因是闭包可能把捕获的数据 move 到异步块中；调用一次后，那些数据已经被消费，不能再调用第二次。
    ///
    /// 这里确实只调用一次：
    ///
    /// ```ignore
    /// operation(Arc::clone(&task)).await
    /// ```
    ///
    /// # 第六部分：闭包返回的 Future 类型
    ///
    /// ```ignore
    /// Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>
    /// ```
    ///
    /// 建议从最里面向外读：
    ///
    /// 1. `Future`
    ///
    ///    表示一个尚未完成、以后可以通过 `.await` 推进的异步计算。
    ///
    /// 2. `Future<Output = Result<(), Error>>`
    ///
    ///    `Output` 是 `Future` trait 的关联类型。它规定 Future 完成后产生：
    ///
    ///    ```ignore
    ///    Result<(), Error>
    ///    ```
    ///
    ///    也就是成功时没有额外值 `Ok(())`，失败时返回 `Error`。
    ///
    /// 3. `dyn Future<...>`
    ///
    ///    `async move { ... }` 会产生一个编译器生成的匿名 Future 类型。
    ///    不同闭包产生的 Future 具体类型不同，`dyn Future` 把具体类型擦除成统一的 trait object。
    ///
    /// 4. `Box<dyn Future<...>>`
    ///
    ///    `dyn Future` 的具体大小在编译期未知，不能直接按值返回。
    ///    `Box` 把它放到堆上；Box 指针本身大小固定，因此可以作为统一返回类型。
    ///
    /// 5. `Pin<Box<...>>`
    ///
    ///    Future 在被轮询后可能包含指向自身内部数据的引用，不能再随意移动。
    ///    `Pin` 表达“这个堆上的 Future 地址固定”，因此可以安全地 `.await`。
    ///
    /// 6. `+ Send`
    ///
    ///    表示这个 Future 可以安全地在线程之间移动，符合 Tokio 多线程运行时的使用要求。
    ///
    /// 7. `+ 'a`
    ///
    ///    表示这个 Future trait object 内部如果持有引用，这些引用至少在 `'a` 范围内有效。
    ///    它把 Future 可借用数据的时间范围和 `self: &'a Arc<Self>` 的借用联系起来。
    ///
    /// 把整个约束翻译成一句普通话：
    ///
    /// ```text
    /// F 是一个最多调用一次的闭包；
    /// 它接收 Arc<Task>；
    /// 返回一个固定在堆上的、可以跨线程移动的异步任务；
    /// 这个异步任务完成后得到 Result<(), Error>。
    /// ```
    ///
    /// `#[allow(clippy::needless_lifetimes)]` 只关闭这个函数上的一条 Clippy 提示。
    /// 这里保留显式 `'a`，是为了把 self 借用和 Future trait object 的生命周期关系写清楚。
    #[allow(clippy::needless_lifetimes)]
    async fn run_task_transition<'a, F>(
        // 显式 self 接收器：借用 Arc<Manager>，生命周期名为 'a。
        self: &'a Arc<Self>,
        task_id: u32,
        action: PendingAction,
        // F 是闭包的具体匿名类型；真正约束写在下面的 where 子句。
        operation: F,
        action_label: &str,
    ) -> Result<u32, Error>
    where
        // `FnOnce(Arc<Task>)`：闭包接收一个 Arc<Task>，并且只保证可以调用一次。
        // 箭头右边是闭包的返回类型，不是 run_task_transition 自身的返回类型。
        F: FnOnce(
            Arc<Task>,
        )
            -> Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>>,
    {
        // 这一句按运算顺序拆开是：
        //
        // 1. 调用 async 函数：
        //    acquire_task_permit_or_queue(self, task_id, action)
        //
        // 2. `.await` 等待，得到：
        //    Result<Option<OwnedSemaphorePermit>, Error>
        //
        // 3. `?` 处理 Result：
        //    - Err(error)     -> 当前函数立刻返回 Err(error)
        //    - Ok(option)     -> 取出内部 Option，继续执行
        //
        // 4. `let Some(permit) = option else { ... }` 处理 Option：
        //    - Some(permit)   -> 把内部 permit 绑定到局部变量 `permit`
        //    - None           -> 执行 else，返回 Ok(task_id)
        //
        // None 表示没有并发名额，queue 模块已经把 action 放进等待队列，
        // 所以这里不是错误，只是不立即执行任务。
        let Some(permit) = acquire_task_permit_or_queue(self, task_id, action).await? else {
            return Ok(task_id);
        };

        // 这一段也可以分层阅读：
        //
        // self.tasks.get(&task_id)
        //     -> Option<DashMap 的只读 guard>
        //
        // .ok_or(Error::TaskNotFound(task_id))
        //     -> 把 None 转成 Err，把 Some 保持为 Ok
        //
        // ?
        //     -> Err 时提前返回；Ok 时取出 guard，绑定给 task_ref
        let task_ref = self
            .tasks
            .get(&task_id)
            .ok_or(Error::TaskNotFound(task_id))?;
        // task_ref.value() 得到 &Arc<Task>；Arc::clone 增加引用计数，得到独立 Arc<Task>。
        // 克隆的不是整个 Task。
        let task = Arc::clone(task_ref.value());

        debug!(
            "[Manager] {} task {} (permits left after acquire = {})",
            action_label,
            task_id,
            self.semaphore.available_permits()
        );

        // 这句严格按顺序执行：
        //
        // Arc::clone(&task)
        //     得到一个新的 Arc<Task> 所有者
        //
        // operation(...)
        //     调用 FnOnce 闭包；闭包本身在这次调用中被消费
        //     返回 Pin<Box<dyn Future<Output = Result<(), Error>>>>
        //
        // .await
        //     等待这个 Future，最终得到 Result<(), Error>
        let res = operation(Arc::clone(&task)).await;
        // match 消费 res，并分别匹配 Result 的两个变体。
        match res {
            // `()` 是单元类型，表示操作成功但没有额外业务返回值。
            Ok(()) => {
                debug!("[Manager] Task {} {} successfully", task_id, action_label);
                // lock().await 得到 MutexGuard<Option<OwnedSemaphorePermit>>。
                // `mut guard` 允许修改 guard 指向的 Option。
                let mut guard = task.permit.lock().await;
                // `*guard` 解引用 MutexGuard，访问内部 Option。
                // `Some(permit)` 把 permit 的所有权移动进 Task；之后局部变量 permit 不再可用。
                // 只要这个 Option 仍持有 permit，Semaphore 的并发名额就一直被占用。
                *guard = Some(permit);
                Ok(task_id)
            }
            // `e` 接收 Error 的所有权，因为 match 正在消费 res。
            Err(e) => {
                error!(
                    "[Manager] Task {} {} failed: {:?}",
                    task_id, action_label, e
                );
                // permit 没有移动进 Task。
                // `drop(permit)` 是显式消费 permit，让其析构函数立即归还 Semaphore 名额。
                drop(permit);
                let mut guard = task.permit.lock().await;
                // 把任务中可能残留的 permit 清空；旧值如果存在，也会在赋值时被 drop。
                *guard = None;

                // `if let Err(spawn_err) = ...` 只处理失败分支；成功的 Ok(()) 被忽略。
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
