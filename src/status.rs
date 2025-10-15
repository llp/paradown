use crate::DownloadError;
use std::fmt;
use std::str::FromStr;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DownloadStatus {
    Pending,
    Preparing,
    Running,
    Paused,
    Completed,
    Canceled,
    Failed(DownloadError),
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
            //TODO
            "Failed" => Ok(DownloadStatus::Failed(crate::error::DownloadError::Other(
                String::from(""),
            ))),
            "Canceled" => Ok(DownloadStatus::Canceled),
            _ => Err(()),
        }
    }
}

