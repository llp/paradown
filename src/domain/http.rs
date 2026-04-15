use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResourceIdentity {
    pub resolved_url: Option<String>,
    pub entity_tag: Option<String>,
    pub last_modified: Option<String>,
}

impl HttpResourceIdentity {
    pub fn resume_validator(&self) -> Option<&str> {
        self.entity_tag.as_deref().or(self.last_modified.as_deref())
    }

    pub fn has_resume_validator(&self) -> bool {
        self.resume_validator().is_some()
    }

    pub fn validator_changed(&self, fresh: &Self) -> bool {
        match (self.entity_tag.as_deref(), fresh.entity_tag.as_deref()) {
            (Some(current), Some(next)) => current != next,
            (Some(_), None) => true,
            _ => match (
                self.last_modified.as_deref(),
                fresh.last_modified.as_deref(),
            ) {
                (Some(current), Some(next)) => current != next,
                (Some(_), None) => true,
                _ => false,
            },
        }
    }
}
