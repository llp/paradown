use crate::Error;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Status {
    Pending,
    Preparing,
    Running,
    Paused,
    Completed,
    Canceled,
    Failed(Error),
    Deleted,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Status::Pending => "Pending",
            Status::Preparing => "Preparing",
            Status::Running => "Running",
            Status::Paused => "Paused",
            Status::Completed => "Completed",
            Status::Failed(_) => "Failed",
            Status::Canceled => "Canceled",
            Status::Deleted => "Deleted",
        };
        write!(f, "{}", s)
    }
}

impl Status {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Status::Completed | Status::Canceled | Status::Failed(_) | Status::Deleted
        )
    }
}

impl FromStr for Status {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(Status::Pending),
            "Running" => Ok(Status::Running),
            "Preparing" => Ok(Status::Preparing),
            "Paused" => Ok(Status::Paused),
            "Completed" => Ok(Status::Completed),
            "Failed" => Ok(Status::Failed(crate::error::Error::Other(String::from("")))),
            "Canceled" => Ok(Status::Canceled),
            "Deleted" => Ok(Status::Deleted),
            _ => Err(()),
        }
    }
}
