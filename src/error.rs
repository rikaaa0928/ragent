use openresponses_rust::StreamingError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error("Streaming error: {0}")]
    Streaming(Box<StreamingError>),

    #[error("Model response failed: {0}")]
    ResponseFailed(String),

    #[error("Environment variable error: {0}")]
    EnvError(#[from] std::env::VarError),

    #[error("Tool error: {0}")]
    ToolError(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Channel send error: {0}")]
    ChannelError(String),

    #[error("Invalid session id: {0}")]
    InvalidSessionId(String),

    #[error("JSON serialization/deserialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Maximum iterations reached ({0})")]
    MaxIterationsReached(usize),

    #[error("Hook '{hook}' rejected the operation: {reason}")]
    HookRejected { hook: String, reason: String },
}

impl From<StreamingError> for AgentError {
    fn from(err: StreamingError) -> Self {
        AgentError::Streaming(Box::new(err))
    }
}
