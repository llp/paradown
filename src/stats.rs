use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct WorkerStats {
    pub id: u32,
    pub downloaded_bytes: u64,
    pub last_update: Instant,
    pub current_speed_bps: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DownloadStatsSnapshot {
    pub total_bytes: u64,
    pub total_speed_bps: u64,
    pub average_speed_bps: u64,
    pub elapsed: f64,
    pub worker_count: usize,
}

pub struct DownloadStats {
    pub created_at: SystemTime,
    pub started_at: RwLock<Option<Instant>>,
    total_bytes: AtomicU64,
    workers: RwLock<HashMap<u32, WorkerStats>>,
    successful_downloads: AtomicU64,
    failed_downloads: AtomicU64,
    retry_count: AtomicU64,
}

impl DownloadStats {
    pub fn new() -> Self {
        Self {
            created_at: SystemTime::now(), // 任务创建时间
            started_at: RwLock::new(None), // 下载实际开始时间
            total_bytes: AtomicU64::new(0),
            workers: RwLock::new(HashMap::new()),
            successful_downloads: AtomicU64::new(0),
            failed_downloads: AtomicU64::new(0),
            retry_count: AtomicU64::new(0),
        }
    }

    pub async fn mark_started(&self) {
        let mut started = self.started_at.write().await;
        if started.is_none() {
            *started = Some(Instant::now());
        }
    }

    pub fn record_success(&self) {
        self.successful_downloads.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.failed_downloads.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_retry(&self) {
        self.retry_count.fetch_add(1, Ordering::Relaxed);
    }

    pub async fn update_worker(&self, worker_id: u32, bytes: u64) {
        {
            let started = self.started_at.read().await;
            if started.is_none() {
                drop(started);
                let mut started = self.started_at.write().await;
                *started = Some(Instant::now());
            }
        }

        let now = Instant::now();
        self.total_bytes.fetch_add(bytes, Ordering::Relaxed);

        let mut workers = self.workers.write().await;
        workers
            .entry(worker_id)
            .and_modify(|w| {
                w.downloaded_bytes += bytes;
                let elapsed = now.duration_since(w.last_update).as_secs_f64().max(0.001);
                let instant_speed = (bytes as f64 / elapsed) as u64;
                w.current_speed_bps =
                    (w.current_speed_bps as f64 * 0.8 + instant_speed as f64 * 0.2) as u64;
                w.last_update = now;
            })
            .or_insert(WorkerStats {
                id: worker_id,
                downloaded_bytes: bytes,
                last_update: now,
                current_speed_bps: 0,
            });
    }

    pub async fn snapshot(&self) -> DownloadStatsSnapshot {
        let workers = self.workers.read().await;
        let total_speed = workers.values().map(|w| w.current_speed_bps).sum::<u64>();
        let total_bytes = self.total_bytes.load(Ordering::Relaxed);

        let elapsed = {
            let started = self.started_at.read().await;
            match *started {
                Some(t) => t.elapsed().as_secs_f64().max(0.001),
                None => 0.0,
            }
        };

        let average_speed = if elapsed > 0.0 {
            (total_bytes as f64 / elapsed) as u64
        } else {
            0
        };

        // 格式化字段
        let formatted_total_bytes = format_bytes(total_bytes);
        let formatted_avg_speed = format_bytes(average_speed);
        let formatted_elapsed = format_duration(elapsed);

        debug!("================= [DownloadStats Snapshot] =================");
        debug!(
            "Task Summary: total={}  avg_speed={}/s  elapsed={}  workers={}",
            formatted_total_bytes,
            formatted_avg_speed,
            formatted_elapsed,
            workers.len()
        );
        if workers.is_empty() {
            debug!("No active workers.");
        } else {
            for (id, w) in workers.iter() {
                debug!(
                    "  Worker #{id}: downloaded={}  speed={}/s  last_update=+{:.2?}",
                    format_bytes(w.downloaded_bytes),
                    format_bytes(w.current_speed_bps),
                    w.last_update.elapsed(),
                );
            }
        }
        debug!("============================================================");

        DownloadStatsSnapshot {
            total_bytes,
            total_speed_bps: total_speed,
            average_speed_bps: average_speed,
            elapsed,
            worker_count: workers.len(),
        }
    }

    pub fn debug_summary(&self) -> String {
        format!(
            "Stats: total={}B | success={} | failed={} | retries={}",
            self.total_bytes.load(Ordering::Relaxed),
            self.successful_downloads.load(Ordering::Relaxed),
            self.failed_downloads.load(Ordering::Relaxed),
            self.retry_count.load(Ordering::Relaxed)
        )
    }
}

/// 将字节数格式化为更友好的字符串（B / KB / MB / GB）
fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// 将秒数格式化为 s 或 m:s 形式
fn format_duration(secs: f64) -> String {
    if secs < 60.0 {
        format!("{:.2}s", secs)
    } else {
        let m = (secs / 60.0).floor();
        let s = secs % 60.0;
        format!("{:.0}m {:.1}s", m, s)
    }
}
