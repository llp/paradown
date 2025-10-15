mod checksum;
mod cli;
mod config;
mod error;
mod events;
mod manager;
mod persistence;
mod progress;
mod repository;
mod request;
mod stats;
mod status;
mod task;
mod worker;

use crate::checksum::DownloadChecksum;
use crate::cli::InteractiveMode;
use crate::config::{
    DownloadConfig, DownloadConfigBuilder, FileConflictStrategy, ProgressThrottleConfig,
    RetryConfig,
};
use crate::error::DownloadError;
use crate::events::DownloadEvent;
use crate::manager::DownloadManager;
use crate::persistence::PersistenceType;
use crate::request::DownloadTaskRequest;
use crate::status::DownloadStatus;
use clap::Parser;
use log::{LevelFilter, error, info};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "paradown")]
#[command(about = "A multi-threaded download tool", long_about = None)]
struct Cli {
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(short, long, default_value_t = 4)]
    workers: usize,

    #[arg(short, long, value_name = "DIR")]
    download_dir: PathBuf,

    #[arg(short, long)]
    shuffle: bool,

    #[arg(short, long)]
    verbose: bool,

    #[arg(short = 'u', long = "urls", value_name = "URLS", num_args = 1.., required = true)]
    urls: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), DownloadError> {
    let cli = Cli::parse();
    let log_level = if cli.verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    env_logger::Builder::from_default_env()
        .filter_level(log_level)
        .filter_module("sqlx::query", LevelFilter::Info) // 只让 sqlx 输出 info 级别以上
        .init();

    let config = match cli.config {
        Some(path) => DownloadConfig::from_file(&path)?,
        None => DownloadConfigBuilder::new()
            .download_dir(cli.download_dir)
            .shuffle(cli.shuffle)
            .urls(cli.urls.clone())
            .worker_threads(cli.workers)
            .max_concurrent_downloads(cli.workers)
            .rate_limit_kbps(None)
            .connection_timeout(30)
            .retry(RetryConfig::default())
            .file_conflict_strategy(FileConflictStrategy::Overwrite)
            .persistence_type(PersistenceType::Sqlite("./downloads/downloads.db".into()))
            .progress_throttle(ProgressThrottleConfig::default())
            .build()?,
    };

    info!(
        "Starting download process with {} workers",
        config.worker_threads
    );
    info!("Download directory: {}", config.download_dir.display());
    info!("Random order: {}", config.shuffle);

    let (mut interactive, status_tx, command_rx) = InteractiveMode::new();
    let manager = DownloadManager::new(config, Some(status_tx.clone()), Some(command_rx))?;
    manager.init().await?;

    // let task_progress_manager = Arc::new(DownloadProgressManager::new());

    for url in cli.urls {
        let request = DownloadTaskRequest::builder(url).build();
        let task_id = manager.add_task(request).await?;
        manager.start_task(task_id).await?;
    }

    let rx = manager.subscribe_events();
    tokio::spawn(async move {
        let mut rx = rx;
        while let Ok(event) = rx.recv().await {
            match event {
                DownloadEvent::Start(id) => {
                    error!("任务 {id} 开始");
                    // let task = task_progress_manager.add_task(id);
                    // task.set_status("Downloading");
                }
                DownloadEvent::Progress {
                    id,
                    downloaded,
                    total,
                } => {
                    // error!("任务 {id} 进度: {downloaded}/{total}");
                    // if let Some(task) = task_progress_manager.get_task(id) {
                    //     task.update(downloaded, Some(total));
                    // } else {
                    //     eprintln!("⚠️ 进度事件丢失: 任务 {id}");
                    // }
                }
                DownloadEvent::Complete(id) => {
                    error!("任务 {id} 已完成");
                    // if let Some(task) = task_progress_manager.get_task(id) {
                    //     task.finish();
                    // } else {
                    //     eprintln!("⚠️ 进度事件丢失: 任务 {id}");
                    // }
                }
                DownloadEvent::Error(id, err) => {
                    error!("任务 {id} 出错: {err:?}");
                    // if let Some(task) = task_progress_manager.get_task(id) {
                    //     task.set_status("Error");
                    // } else {
                    //     eprintln!("⚠️ 进度事件丢失: 任务 {id}");
                    // }
                }
                _ => {}
            }
        }
    });

    //-------------------------start---------------------------
    // tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    // manager.start_all().await?;
    //-------------------------Pause & Resume---------------------------
    // tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    // manager.pause_all().await?;

    // tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    // manager.resume_all().await?;

    //-------------------------Pause & Resume---------------------------
    // tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    //
    // if let Some(t) = manager.get_task_by_id(1) {
    //     println!("[Main] Pausing task...");
    //     if let Err(e) = t.pause().await {
    //         eprintln!("[Main] Failed to pause task: {:?}", e);
    //     } else {
    //         println!("[Main] Task paused successfully");
    //     }
    // }
    //
    // tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    //
    // if let Some(t) = manager.get_task_by_id(1) {
    //     println!("[Main] Resuming task...");
    //     if let Err(e) = t.resume().await {
    //         eprintln!("[Main] Failed to resume task: {:?}", e);
    //     } else {
    //         println!("[Main] Task resumed successfully");
    //     }
    // }
    //------------------------add_task---------------------------------
    // tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
    //
    // warn!("Add new task ...");
    // let request = DownloadRequest {
    //     url: "https://extcdn.hsrc.tv/channelzero/video/zero_public/2024/03/04/26405d75acd94645b117cd075a0b5c89_transcode_1137855.mp4".to_string(),
    //     file_name: None,
    //     checksums: vec![
    //         DownloadChecksum {
    //             algorithm: ChecksumAlgorithm::MD5,
    //             value: Some("9d15d120a60995be4f66a30ff66be684".to_string()),
    //         }
    //     ],
    // };
    //
    // let request = DownloadRequest {
    //     url: "https://extcdn.hsrc.tv/channelzero/video/zero_public/2024/03/01/96e83f0c3cce4c8c952a5334c41252c9_transcode_1137855.mp4".to_string(),
    //     file_name: None,
    //     checksums: vec![
    //         DownloadChecksum {
    //             algorithm: ChecksumAlgorithm::MD5,
    //             value: Some("9020e80bdf8012d15f75ca57b01ab862".to_string()),
    //         }
    //     ],
    // };
    //
    // let task_id = manager.add_task(request).await?;
    // manager.start_task(task_id).await?;

    //----------------------------------------------------------------
    let manager_clone = Arc::clone(&manager);
    tokio::spawn(async move {
        if let Err(e) = manager_clone.run().await {
            eprintln!("Error in manager run: {:?}", e);
        }
    });

    interactive.run().await;

    info!("Download process completed successfully");
    Ok(())
}
