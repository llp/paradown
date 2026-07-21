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
    next_available_at: Mutex<Instant>,
    update_tx: watch::Sender<()>,
}

impl DownloadRateLimiter {
    pub(crate) fn new(limit_kib_per_sec: Option<NonZeroU64>) -> Self {
        let (update_tx, _update_rx) = watch::channel(());
        Self {
            bytes_per_second: AtomicU64::new(kib_to_bytes(limit_kib_per_sec)),
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
                // 用一个较小作用域包住锁守卫，让锁在进入 `select!` 前释放。
                // 这样等待 sleep 或配置变化时，不会一直占着 Mutex。
                let mut next_available_at = self.next_available_at.lock().await;
                let now = Instant::now();
                let slot = (*next_available_at).max(now);
                *next_available_at = slot + reservation;
                slot
            };

            // `tokio::select!` 同时等待多个异步分支，先完成的分支获胜。
            // 这里要么等到预约时间，要么等到限速配置变化后重新计算。
            tokio::select! {
                _ = tokio::time::sleep_until(wait_until) => return,
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
