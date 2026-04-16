use crate::domain::{DownloadSpec, SourceSet};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMode {
    SingleSource,
    MultiSource,
    Swarm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDescriptor {
    pub spec: DownloadSpec,
    pub sources: SourceSet,
    pub mode: SessionMode,
}

impl SessionDescriptor {
    pub fn new(spec: DownloadSpec, sources: SourceSet) -> Self {
        let mode = if sources.active_transfer_sources().len() > 1 {
            SessionMode::MultiSource
        } else {
            SessionMode::SingleSource
        };

        Self {
            spec,
            sources,
            mode,
        }
    }
}
