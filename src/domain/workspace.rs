use crate::domain::ids::WorkspaceId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSpec {
    pub format_version: u32,
    pub id: WorkspaceId,
    pub root: PathBuf,
    pub created_at: DateTime<Utc>,
}

impl WorkspaceSpec {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let canonical_root = root.as_ref().canonicalize()?;
        let id_str = format!("ws_{}", uuid::Uuid::now_v7().simple());
        Ok(Self {
            format_version: 1,
            id: WorkspaceId::new(id_str),
            root: canonical_root,
            created_at: Utc::now(),
        })
    }

    pub fn with_id(id: WorkspaceId, root: impl AsRef<Path>) -> std::io::Result<Self> {
        let canonical_root = root.as_ref().canonicalize()?;
        Ok(Self {
            format_version: 1,
            id,
            root: canonical_root,
            created_at: Utc::now(),
        })
    }
}
