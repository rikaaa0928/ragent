use crate::domain::batch::ItemBatch;
use crate::domain::event::{SessionEvent, SessionEventEnvelope};
use crate::domain::ids::{BatchSeq, EventSeq, LocalItemSeq};
use crate::domain::session::{SessionPhase, SessionSpec, SessionStatus};

#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    #[error("Empty events for session {0}")]
    EmptyEvents(String),
    #[error("Event sequence discontinuity: expected {expected}, found {found}")]
    EventDiscontinuity { expected: u64, found: u64 },
    #[error("Batch sequence discontinuity: expected {expected}, found {found}")]
    BatchDiscontinuity { expected: u64, found: u64 },
    #[error("Item sequence discontinuity: expected {expected}, found {found}")]
    ItemDiscontinuity { expected: u64, found: u64 },
    #[error("First event is not SessionCreated")]
    FirstEventNotCreated,
}

pub fn rebuild_session_status_from_facts(
    spec: &SessionSpec,
    batches: &[ItemBatch],
    events: &[SessionEventEnvelope],
) -> Result<SessionStatus, ProjectionError> {
    if events.is_empty() {
        return Err(ProjectionError::EmptyEvents(spec.id.to_string()));
    }

    // 1. Verify event sequence continuity
    for (i, env) in events.iter().enumerate() {
        let expected_seq = EventSeq::new(i as u64);
        if env.event_seq != expected_seq {
            return Err(ProjectionError::EventDiscontinuity {
                expected: expected_seq.as_u64(),
                found: env.event_seq.as_u64(),
            });
        }
    }

    // 2. Verify batch sequence continuity and item sequence continuity
    let mut expected_item_seq = LocalItemSeq::ZERO;
    for (i, batch) in batches.iter().enumerate() {
        let expected_batch_seq = BatchSeq::new(i as u64);
        if batch.batch_seq != expected_batch_seq {
            return Err(ProjectionError::BatchDiscontinuity {
                expected: expected_batch_seq.as_u64(),
                found: batch.batch_seq.as_u64(),
            });
        }
        if batch.first_local_item_seq != expected_item_seq {
            return Err(ProjectionError::ItemDiscontinuity {
                expected: expected_item_seq.as_u64(),
                found: batch.first_local_item_seq.as_u64(),
            });
        }
        expected_item_seq =
            LocalItemSeq::new(batch.first_local_item_seq.as_u64() + batch.items.len() as u64);
    }

    // 3. Replay events
    let mut phase = SessionPhase::Open;
    let mut active_activation_id = None;
    let mut last_error = None;
    let mut updated_at = spec.created_at;

    for (i, env) in events.iter().enumerate() {
        updated_at = env.created_at;
        if i == 0 {
            if !matches!(env.event, SessionEvent::SessionCreated { .. }) {
                return Err(ProjectionError::FirstEventNotCreated);
            }
            continue;
        }

        match &env.event {
            SessionEvent::SessionCreated { .. } => {}
            SessionEvent::SessionClosed => {
                phase = SessionPhase::Closed;
            }
            SessionEvent::SessionArchived => {
                phase = SessionPhase::Archived;
            }
            SessionEvent::ActivationRequested { .. } | SessionEvent::ActivationStarted => {
                active_activation_id = env.activation_id.clone();
            }
            SessionEvent::TurnStarted | SessionEvent::TurnCompleted { .. } => {}
            SessionEvent::ToolCallStarted { .. } | SessionEvent::ToolCallFinished { .. } => {}
            SessionEvent::ActivationCompleted { .. } => {
                active_activation_id = None;
            }
            SessionEvent::ActivationFailed { error } => {
                active_activation_id = None;
                last_error = Some(error.clone());
            }
            SessionEvent::ActivationCancelled => {
                active_activation_id = None;
            }
            SessionEvent::ActivationInterrupted { reason } => {
                active_activation_id = None;
                last_error = Some(reason.clone());
            }
        }
    }

    let local_item_count: u64 = batches.iter().map(|b| b.items.len() as u64).sum();

    Ok(SessionStatus {
        projection_version: 1,
        phase,
        active_activation_id,
        queued_activation_count: 0,
        local_item_count,
        effective_context_item_count: local_item_count,
        batch_count: batches.len() as u64,
        event_count: events.len() as u64,
        updated_at,
        title: None,
        last_error,
    })
}
