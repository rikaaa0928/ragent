pub mod lock;
pub mod runner;
pub mod service;

use crate::domain::event::SessionEventEnvelope;
use std::sync::Arc;

pub type EventCallback = Arc<dyn Fn(&SessionEventEnvelope) + Send + Sync>;

pub use runner::*;
pub use service::*;
