use crate::domain::ids::{
    ActivationId, BatchSeq, CommandId, ConfigRef, EventId, EventSeq, SessionId, TurnId,
};
use chrono::{DateTime, Utc};
use openresponses_rust::Usage;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEventEnvelope {
    pub format_version: u32,
    pub event_seq: EventSeq,
    pub event_id: EventId,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation_id: Option<ActivationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub created_at: DateTime<Utc>,
    pub event: SessionEvent,
}

impl SessionEventEnvelope {
    pub fn new(
        event_seq: EventSeq,
        session_id: SessionId,
        activation_id: Option<ActivationId>,
        turn_id: Option<TurnId>,
        event: SessionEvent,
    ) -> Self {
        Self {
            format_version: 1,
            event_seq,
            event_id: EventId::generate(),
            session_id,
            activation_id,
            turn_id,
            created_at: Utc::now(),
            event,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    SessionCreated {
        #[serde(skip_serializing_if = "Option::is_none")]
        command_id: Option<CommandId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    SessionClosed,
    SessionArchived,
    ActivationRequested {
        config_ref: ConfigRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        command_id: Option<CommandId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
        input_batch_seq: BatchSeq,
    },
    ActivationStarted,
    TurnStarted,
    TurnCompleted {
        #[serde(skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_batch_seq: Option<BatchSeq>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    ToolCallStarted {
        call_id: String,
        name: String,
    },
    ToolCallFinished {
        call_id: String,
        name: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        output_batch_seq: BatchSeq,
    },
    ActivationCompleted {
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    ActivationFailed {
        error: String,
    },
    ActivationCancelled,
    ActivationInterrupted {
        reason: String,
    },
}

impl SessionEvent {
    pub fn kind_str(&self) -> &'static str {
        match self {
            SessionEvent::SessionCreated { .. } => "session_created",
            SessionEvent::SessionClosed => "session_closed",
            SessionEvent::SessionArchived => "session_archived",
            SessionEvent::ActivationRequested { .. } => "activation_requested",
            SessionEvent::ActivationStarted => "activation_started",
            SessionEvent::TurnStarted => "turn_started",
            SessionEvent::TurnCompleted { .. } => "turn_completed",
            SessionEvent::ToolCallStarted { .. } => "tool_call_started",
            SessionEvent::ToolCallFinished { .. } => "tool_call_finished",
            SessionEvent::ActivationCompleted { .. } => "activation_completed",
            SessionEvent::ActivationFailed { .. } => "activation_failed",
            SessionEvent::ActivationCancelled => "activation_cancelled",
            SessionEvent::ActivationInterrupted { .. } => "activation_interrupted",
        }
    }
}
