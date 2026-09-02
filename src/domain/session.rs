use crate::domain::ids::{
    ActivationId, CommandId, ConfigRef, ContextPos, SessionId, TurnId, WorkspaceId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const DEFAULT_SYSTEM_PROMPT: &str =
    "你是一个高效、精准、善于深度思考的 AI 智能体助手。你可以通过工具感知环境并完成复杂任务。";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionSpec {
    pub format_version: u32,
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub basic_system_prompt: String,
    pub default_config_ref: ConfigRef,
    pub workspace_ref: WorkspaceId,
    #[serde(default)]
    pub sources: Vec<SessionSource>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerRef>,
}

impl SessionSpec {
    pub fn new(
        id: SessionId,
        basic_system_prompt: impl Into<String>,
        default_config_ref: ConfigRef,
        workspace_ref: WorkspaceId,
    ) -> Self {
        Self {
            format_version: 1,
            id,
            created_at: Utc::now(),
            basic_system_prompt: basic_system_prompt.into(),
            default_config_ref,
            workspace_ref,
            sources: Vec::new(),
            labels: BTreeMap::new(),
            producer: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSource {
    pub kind: SessionSourceKind,
    pub session_id: SessionId,
    pub from_context_pos: ContextPos,
    pub through_context_pos: ContextPos,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionSourceKind {
    ForkedFrom,
    DerivedFrom,
    SummaryOf,
    ContinuedFrom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProducerRef {
    pub session_id: SessionId,
    pub activation_id: ActivationId,
    pub turn_id: Option<TurnId>,
    pub call_id: Option<String>,
    pub command_id: CommandId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Open,
    Closed,
    Archived,
    Corrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionStatus {
    pub projection_version: u32,
    pub phase: SessionPhase,
    pub active_activation_id: Option<ActivationId>,
    pub queued_activation_count: usize,
    pub local_item_count: u64,
    pub effective_context_item_count: u64,
    pub batch_count: u64,
    pub event_count: u64,
    pub updated_at: DateTime<Utc>,
    pub title: Option<String>,
    pub last_error: Option<String>,
}

impl SessionStatus {
    pub fn initial() -> Self {
        Self {
            projection_version: 1,
            phase: SessionPhase::Open,
            active_activation_id: None,
            queued_activation_count: 0,
            local_item_count: 0,
            effective_context_item_count: 0,
            batch_count: 0,
            event_count: 1, // SessionCreated event
            updated_at: Utc::now(),
            title: None,
            last_error: None,
        }
    }
}

pub fn session_tmp_dir(session_id: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/ragent/{}", session_id))
}

pub fn ensure_session_tmp_dir(session_id: &str) -> Result<PathBuf, String> {
    crate::domain::ids::validate_session_id(session_id)?;
    let dir = session_tmp_dir(session_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create session temp dir at {:?}: {}", dir, e))?;
    Ok(dir)
}

pub fn build_basic_system_prompt(session_id: &str) -> Result<String, String> {
    let tmp_dir = ensure_session_tmp_dir(session_id)?;
    let mut prompt = String::from(DEFAULT_SYSTEM_PROMPT);
    prompt.push_str("\n\n## 会话临时目录 (session_tmp)\n");
    prompt.push_str(&format!(
        "系统已为当前会话分配了专用临时目录：`{}`。\n\
         - 你可以在此目录读写中间数据、临时脚本或生成文件。\n\
         - 该目录隔离于项目代码区，避免污染项目工作树。\n",
        tmp_dir.display()
    ));
    Ok(prompt)
}
