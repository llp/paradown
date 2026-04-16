use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, watch};
use tokio::time::{Duration, Instant};

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

    pub(crate) async fn set_limit_kib_per_sec(&self, limit_kib_per_sec: Option<NonZeroU64>) {
        self.bytes_per_second
            .store(kib_to_bytes(limit_kib_per_sec), Ordering::Relaxed);
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
                let mut next_available_at = self.next_available_at.lock().await;
                let now = Instant::now();
                let slot = (*next_available_at).max(now);
                *next_available_at = slot + reservation;
                slot
            };

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
