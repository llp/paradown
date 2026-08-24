use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, watch};
use tokio::time::{Duration, Instant};

/// 下载限速器。
///
/// 这个结构体是学习 Rust 并发原语的代表位置：
/// - `AtomicU64` 用于无锁保存当前限速值。
/// - `Mutex<Instant>` 用于保护需要按顺序更新的下一次可用时间。
/// - `watch::Sender<()>` 用于通知等待中的异步任务“配置变了”。
///
/// async 和并发语法见 `docs/rust/async-and-concurrency.md`。
pub(crate) struct DownloadRateLimiter {
    bytes_per_second: AtomicU64,
    /// 下一个允许开始传输的时间点。
    ///
    /// 这里使用 Tokio 的 `Instant`，而不是 `SystemTime`：
    /// - `Instant` 表示单调递增的时间点，适合计算“还要等待多久”。
    /// - `SystemTime` 表示墙上时钟，可能因为系统校时、时区或夏令时发生跳变，
    ///   不适合直接用于超时和间隔计算。
    ///
    /// `Mutex<Instant>` 的含义是：多个异步任务要共同更新这个时间点，
    /// 更新过程必须串行化，否则两个任务可能同时读到同一个旧时间点，预约到同一个时段。
    next_available_at: Mutex<Instant>,
    update_tx: watch::Sender<()>,
}

impl DownloadRateLimiter {
    pub(crate) fn new(limit_kib_per_sec: Option<NonZeroU64>) -> Self {
        let (update_tx, _update_rx) = watch::channel(());
        Self {
            bytes_per_second: AtomicU64::new(kib_to_bytes(limit_kib_per_sec)),
            // `Instant::now()` 创建“当前单调时间点”。
            // 第一个 acquire 可以从现在开始预约，不需要额外等待。
            next_available_at: Mutex::new(Instant::now()),
            update_tx,
        }
    }

    /// 更新限速。
    ///
    /// `&self` 是不可变借用，但仍然可以修改 `AtomicU64` 和 `Mutex` 内部的值。
    /// 这是 Rust 的“内部可变性”模式：外部看是共享引用，内部类型自己保证并发安全。
    pub(crate) async fn set_limit_kib_per_sec(&self, limit_kib_per_sec: Option<NonZeroU64>) {
        self.bytes_per_second
            .store(kib_to_bytes(limit_kib_per_sec), Ordering::Relaxed);
        // `lock().await` 获取 Tokio 异步互斥锁。
        // 前面的 `*` 解引用锁守卫，修改被 Mutex 保护的 Instant。
        // 把时间点重置为现在，表示改变限速后重新计算排队时间。
        *self.next_available_at.lock().await = Instant::now();
        let _ = self.update_tx.send(());
    }

    pub(crate) fn current_limit_kib_per_sec(&self) -> Option<u64> {
        let bytes_per_second = self.bytes_per_second.load(Ordering::Relaxed);
        if bytes_per_second == 0 {
            None
        } else {
            Some(bytes_per_second / 1024)
        }
    }

    /// 按当前速率为 `bytes` 字节预约一个传输时间段。
    ///
    /// 这个函数名叫 `acquire`，但它不是获取 Semaphore permit；这里的“获取”是：
    /// 等到限速器允许当前这批字节开始传输。
    ///
    /// 可以先把算法想成一个按时间排列的队列：
    ///
    /// ```text
    /// next_available_at = 现在
    ///
    /// 第一个任务预约 [现在, 现在 + duration_for(bytes)]
    /// 第二个任务预约 [上一个结束时间, 上一个结束时间 + duration_for(bytes)]
    /// 第三个任务继续排在后面
    /// ```
    ///
    /// 这里没有真的保存一个任务列表，而是只保存“队列尾部的时间点”
    /// `next_available_at`。每次调用 acquire 都把自己的时间段追加到这个时间点后面。
    ///
    /// # `Instant` 的详细用法
    ///
    /// ## 1. `Instant` 是时间点，不是时间长度
    ///
    /// ```ignore
    /// let point: Instant = Instant::now();
    /// let length: Duration = Duration::from_secs(1);
    /// ```
    ///
    /// - `Instant`：某个时间点，例如“现在”或“未来某一时刻”。
    /// - `Duration`：两个时间点之间的长度，例如“一秒”。
    ///
    /// 不能把 `Duration` 当作时间点，也不能把 `Instant` 当作字节需要的秒数。
    ///
    /// ## 2. 为什么用 `tokio::time::Instant`
    ///
    /// 当前文件导入的是：
    ///
    /// ```ignore
    /// use tokio::time::{Duration, Instant};
    /// ```
    ///
    /// Tokio 的 `sleep_until` 也接受 Tokio 的 `Instant`，所以整个异步计时链使用同一个时间类型：
    ///
    /// ```ignore
    /// let deadline = Instant::now() + Duration::from_secs(1);
    /// tokio::time::sleep_until(deadline).await;
    /// ```
    ///
    /// `Instant::now()` 不受系统时间向前或向后调整影响，适合计算间隔、超时和截止时间。
    ///
    /// ## 3. `Instant + Duration`
    ///
    /// ```ignore
    /// let finish = start + reservation;
    /// ```
    ///
    /// 这不是把两个“时间长度”相加，而是从时间点 `start` 往后推 `reservation`，得到新的时间点。
    /// 在本函数中：
    ///
    /// ```ignore
    /// *next_available_at = slot + reservation;
    /// ```
    ///
    /// 表示当前任务的预约结束时间，也就是下一个任务最早可以排到的时间点。
    ///
    /// ## 4. `Instant::max`
    ///
    /// ```ignore
    /// let slot = (*next_available_at).max(now);
    /// ```
    ///
    /// 如果队列尾部时间已经过去，就从现在开始；如果队列尾部还在未来，就继续排队。
    ///
    /// | 队列尾部 | 当前时间 | `max` 结果 | 含义 |
    /// | --- | --- | --- | --- |
    /// | 过去 | 现在 | 现在 | 立即开始预约 |
    /// | 未来 | 现在 | 未来 | 等待前面的预约结束 |
    ///
    /// ## 5. `sleep_until`
    ///
    /// ```ignore
    /// tokio::time::sleep_until(wait_until).await
    /// ```
    ///
    /// 它等待的是“直到某个时间点”，而不是“睡眠固定多少秒”。
    /// 如果任务被调度得稍晚，Tokio 会根据绝对截止时间判断是否已经到点。
    ///
    /// # `acquire` 的执行步骤
    ///
    /// 1. `bytes == 0` 时直接返回：零字节不需要占用传输时间。
    ///
    /// 2. `subscribe()` 创建当前调用的 watch receiver。
    ///    receiver 用来监听限速配置变化；每个 acquire 都有自己独立的接收位置。
    ///
    /// 3. 进入 `loop`，每次循环重新读取速率和重新计算预约时间。
    ///
    /// 4. 如果速率是 0，约定为“不限速”，直接返回。
    ///
    /// 5. `duration_for(bytes, bytes_per_second)` 根据公式计算这批字节需要的时间：
    ///
    ///    ```text
    ///    时间 = 字节数 / 每秒字节数
    ///    ```
    ///
    /// 6. 获取 `next_available_at` 的异步锁，独占地读取和更新队列尾部时间点。
    ///
    /// 7. 通过 `max(now)` 防止已经过期的队列尾部导致额外等待。
    ///
    /// 8. 保存当前任务的起始时间 `slot`，并把队列尾部推进到 `slot + reservation`。
    ///
    /// 9. 释放 Mutex guard。这个作用域非常重要：不能拿着锁去等待 sleep，
    ///    否则后续任务无法预约，限速器会被第一个等待者堵住。
    ///
    /// 10. 使用 `tokio::select!` 同时等待两件事：
    ///     - 到达 `wait_until`：当前预约完成，函数返回。
    ///     - watch 配置变化：当前预约可能已经不合理，循环重新读取速率并重新预约。
    ///
    /// 11. 如果 watch sender 被销毁，`changed()` 返回错误，函数结束。
    ///
    /// # 为什么配置变化后要重新循环
    ///
    /// 假设任务原本按很低的速率预约了未来十秒：
    ///
    /// ```text
    /// wait_until = 10 秒后
    /// ```
    ///
    /// 如果用户此时取消限速，继续睡十秒就不合理了。
    /// `set_limit_kib_per_sec` 会发送 watch 通知，select 选择 `changed` 分支，
    /// acquire 回到循环顶部重新读取速率。限速变为 0 时直接返回；新速率下则重新计算等待。
    pub(crate) async fn acquire(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }

        let mut updates = self.update_tx.subscribe();
        loop {
            let bytes_per_second = self.bytes_per_second.load(Ordering::Relaxed);
            if bytes_per_second == 0 {
                return;
            }

            let reservation = duration_for(bytes, bytes_per_second);
            let wait_until = {
                // 这个局部作用域让 MutexGuard 在 `select!` 之前析构。
                // guard 离开作用域时自动解锁；后面的异步等待期间不持有锁。
                let mut next_available_at = self.next_available_at.lock().await;
                // `Instant::now()` 取得此刻的单调时间点。
                let now = Instant::now();
                // MutexGuard 实现解引用，所以 `*next_available_at` 取得里面的 Instant。
                // `max` 选择队列尾部和当前时间中更晚的那个，得到本次预约起点。
                let slot = (*next_available_at).max(now);
                // Instant 加 Duration 得到新的未来时间点，作为后续任务的队列尾部。
                *next_available_at = slot + reservation;
                // 返回本次任务应该开始等待的绝对时间点。
                slot
            };

            // `select!` 同时等待两个 Future，先完成的分支获胜。
            // 这里等待的是一个“时间点” sleep 和一个“配置变化” Future。
            tokio::select! {
                // 到达预约起点，当前调用获得传输时间，结束 acquire。
                _ = tokio::time::sleep_until(wait_until) => return,
                // 配置变化时不直接返回，而是回到 loop 顶部重新计算。
                changed = updates.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

fn kib_to_bytes(limit_kib_per_sec: Option<NonZeroU64>) -> u64 {
    limit_kib_per_sec
        .map(|value| value.get().saturating_mul(1024))
        .unwrap_or(0)
}

fn duration_for(bytes: u64, bytes_per_second: u64) -> Duration {
    Duration::from_secs_f64(bytes as f64 / bytes_per_second as f64)
}

#[cfg(test)]
mod tests {
    use super::DownloadRateLimiter;
    use std::num::NonZeroU64;
    use std::time::Duration;

    #[tokio::test]
    async fn reports_current_limit_in_kilobytes() {
        let limiter = DownloadRateLimiter::new(NonZeroU64::new(64));
        assert_eq!(limiter.current_limit_kib_per_sec(), Some(64));

        limiter.set_limit_kib_per_sec(None).await;
        assert_eq!(limiter.current_limit_kib_per_sec(), None);
    }

    #[tokio::test]
    async fn wakes_waiters_when_rate_limit_changes() {
        let limiter = std::sync::Arc::new(DownloadRateLimiter::new(NonZeroU64::new(16)));
        let acquire_handle = {
            let limiter = std::sync::Arc::clone(&limiter);
            tokio::spawn(async move {
                limiter.acquire(64 * 1024).await;
            })
        };

        tokio::time::sleep(Duration::from_millis(50)).await;
        limiter.set_limit_kib_per_sec(None).await;

        tokio::time::timeout(Duration::from_millis(250), acquire_handle)
            .await
            .expect("acquire should wake after limit update")
            .expect("acquire task should complete cleanly");
    }
}
