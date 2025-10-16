use crate::DownloadError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DownloadStatus {
    Pending,
    Preparing,
    Running,
    Paused,
    Completed,
    Canceled,
    Failed(DownloadError),
    Deleted,
}

impl fmt::Display for DownloadStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            DownloadStatus::Pending => "Pending",
            DownloadStatus::Preparing => "Preparing",
            DownloadStatus::Running => "Running",
            DownloadStatus::Paused => "Paused",
            DownloadStatus::Completed => "Completed",
            DownloadStatus::Failed(_) => "Failed",
            DownloadStatus::Canceled => "Canceled",
            DownloadStatus::Deleted => "Deleted",
        };
        write!(f, "{}", s)
    }
}

impl FromStr for DownloadStatus {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(DownloadStatus::Pending),
            "Running" => Ok(DownloadStatus::Running),
            "Preparing" => Ok(DownloadStatus::Preparing),
            "Paused" => Ok(DownloadStatus::Paused),
            "Completed" => Ok(DownloadStatus::Completed),
            "Failed" => Ok(DownloadStatus::Failed(crate::error::DownloadError::Other(
                String::from(""),
            ))),
            "Canceled" => Ok(DownloadStatus::Canceled),
            "Deleted" => Ok(DownloadStatus::Deleted),
            _ => Err(()),
        }
    }
}
