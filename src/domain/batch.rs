use crate::domain::ids::{
    ActivationId, BatchSeq, CommandId, ConfigRef, LocalItemSeq, SessionId, TurnId,
};
use chrono::{DateTime, Utc};
use openresponses_rust::Item;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemBatchKind {
    Input,
    ModelOutput,
    ToolOutput,
}

impl ItemBatchKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemBatchKind::Input => "input",
            ItemBatchKind::ModelOutput => "model_output",
            ItemBatchKind::ToolOutput => "tool_output",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivationRequestMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_id: Option<CommandId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    pub config_ref: ConfigRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ItemBatch {
    pub format_version: u32,
    pub session_id: SessionId,
    pub batch_seq: BatchSeq,
    pub first_local_item_seq: LocalItemSeq,
    pub kind: ItemBatchKind,
    pub activation_id: ActivationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation_request: Option<ActivationRequestMeta>,
    pub items: Vec<Item>,
}

impl ItemBatch {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: SessionId,
        batch_seq: BatchSeq,
        first_local_item_seq: LocalItemSeq,
        kind: ItemBatchKind,
        activation_id: ActivationId,
        turn_id: Option<TurnId>,
        response_id: Option<String>,
        activation_request: Option<ActivationRequestMeta>,
        items: Vec<Item>,
    ) -> Result<Self, String> {
        if items.is_empty() {
            return Err("ItemBatch items cannot be empty".into());
        }
        Ok(Self {
            format_version: 1,
            session_id,
            batch_seq,
            first_local_item_seq,
            kind,
            activation_id,
            turn_id,
            response_id,
            created_at: Utc::now(),
            activation_request,
            items,
        })
    }

    pub fn last_local_item_seq(&self) -> LocalItemSeq {
        LocalItemSeq::new(self.first_local_item_seq.as_u64() + self.items.len() as u64 - 1)
    }

    pub fn item_count(&self) -> u64 {
        self.items.len() as u64
    }
}
