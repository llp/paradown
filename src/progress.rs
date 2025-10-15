use bytesize;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 单个下载任务的进度条
pub struct DownloadTaskProgress {
    pub name: String,
    pub total_bytes: Arc<AtomicU64>, // 支持动态更新总长度
    pub downloaded_bytes: AtomicU64,
    pub status: Arc<Mutex<String>>,
    pub progress_bar: ProgressBar,
    pub start_time: Instant,
    pub end_time: Arc<Mutex<Option<Instant>>>,
}

impl DownloadTaskProgress {
    pub fn new(task_id: u32, multi: &MultiProgress) -> Self {
        let pb = multi.add(ProgressBar::new(0)); // 初始总长度未知
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} {msg}")
                .unwrap()
                .progress_chars("=>-"),
        );
        pb.enable_steady_tick(Duration::from_millis(100));

        Self {
            name: format!("Task-{task_id}"),
            total_bytes: Arc::new(AtomicU64::new(0)),
            downloaded_bytes: AtomicU64::new(0),
            status: Arc::new(Mutex::new("Pending".to_string())),
            progress_bar: pb,
            start_time: Instant::now(),
            end_time: Arc::new(Mutex::new(None)),
        }
    }

    pub fn update(&self, downloaded: u64, total: Option<u64>) {
        self.downloaded_bytes.store(downloaded, Ordering::SeqCst);
        if let Some(t) = total {
            self.total_bytes.store(t, Ordering::SeqCst);
            self.progress_bar.set_length(t);
        }
        self.progress_bar.set_position(downloaded);

        let status = self.status.lock().unwrap().clone();
        let total_str = if self.total_bytes.load(Ordering::SeqCst) > 0 {
            bytesize::to_string(self.total_bytes.load(Ordering::SeqCst), true)
        } else {
            "?".to_string()
        };

        let elapsed = self.elapsed();
        let msg = format!(
            "{} | status={} | {}/{} | elapsed={}",
            self.name,
            status,
            bytesize::to_string(downloaded, true),
            total_str,
            humantime::format_duration(elapsed)
        );
        self.progress_bar.set_message(msg);
    }

    pub fn set_status(&self, new_status: &str) {
        let mut status = self.status.lock().unwrap();
        *status = new_status.to_string();
        self.update(self.downloaded_bytes.load(Ordering::SeqCst), None);
        if matches!(new_status, "Completed" | "Error") {
            *self.end_time.lock().unwrap() = Some(Instant::now());
        }
    }

    pub fn finish(&self) {
        self.set_status("Completed");
        self.progress_bar.finish_with_message(format!(
            "{} ✅ Completed in {}",
            self.name,
            humantime::format_duration(self.elapsed())
        ));
    }

    pub fn elapsed(&self) -> Duration {
        match *self.end_time.lock().unwrap() {
            Some(t) => t.duration_since(self.start_time),
            None => Instant::now().duration_since(self.start_time),
        }
    }
}

/// 下载任务管理器：线程安全的任务集合
pub struct DownloadProgressManager {
    pub tasks: Arc<Mutex<HashMap<u32, Arc<DownloadTaskProgress>>>>,
    pub multi_progress: Arc<MultiProgress>,
}

impl DownloadProgressManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            multi_progress: Arc::new(MultiProgress::new()),
        }
    }

    pub fn add_task(&self, task_id: u32) -> Arc<DownloadTaskProgress> {
        let task = Arc::new(DownloadTaskProgress::new(task_id, &self.multi_progress));
        self.tasks.lock().unwrap().insert(task_id, task.clone());
        task
    }

    pub fn get_task(&self, task_id: u32) -> Option<Arc<DownloadTaskProgress>> {
        self.tasks.lock().unwrap().get(&task_id).cloned()
    }
}
