use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationPhase {
    Queued,
    Running,
    WaitingForInteraction,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl ActivationPhase {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ActivationPhase::Succeeded
                | ActivationPhase::Failed
                | ActivationPhase::Cancelled
                | ActivationPhase::Interrupted
        )
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self,
            ActivationPhase::Queued
                | ActivationPhase::Running
                | ActivationPhase::WaitingForInteraction
                | ActivationPhase::Cancelling
        )
    }
}
