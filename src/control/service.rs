use crate::config::{AgentConfig, ModelSettings};
use crate::control::runner::{SessionRunResult, SessionRunner};
use crate::control::EventCallback;
use crate::domain::config::ConfigRevision;
use crate::domain::event::SessionEventEnvelope;
use crate::domain::ids::SessionId;
use crate::domain::session::{build_basic_system_prompt, SessionSpec, SessionStatus};
use crate::domain::workspace::WorkspaceSpec;
use crate::error::AgentError;
use crate::hooks::manager::HookManager;
use crate::store::sqlite::SqliteControlStore;
use openresponses_rust::{CreateResponseBody, Item};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct ControlService {
    store: SqliteControlStore,
    config: AgentConfig,
    config_dir: Option<PathBuf>,
}

impl ControlService {
    pub fn new(store: SqliteControlStore, config: AgentConfig) -> Self {
        Self {
            store,
            config,
            config_dir: None,
        }
    }

    pub fn with_config_dir(mut self, config_dir: impl AsRef<Path>) -> Self {
        self.config_dir = Some(config_dir.as_ref().to_path_buf());
        self
    }

    pub fn open(db_path: impl AsRef<Path>, config: AgentConfig) -> Result<Self, AgentError> {
        let store =
            SqliteControlStore::open(db_path).map_err(|e| AgentError::ToolError(e.to_string()))?;
        Ok(Self {
            store,
            config,
            config_dir: None,
        })
    }

    pub fn open_in_memory(config: AgentConfig) -> Result<Self, AgentError> {
        let store = SqliteControlStore::open_in_memory()
            .map_err(|e| AgentError::ToolError(e.to_string()))?;
        Ok(Self {
            store,
            config,
            config_dir: None,
        })
    }

    pub fn store(&self) -> &SqliteControlStore {
        &self.store
    }

    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    pub fn create_session(
        &self,
        workspace_root: impl AsRef<Path>,
        custom_prompt: Option<&str>,
        model_override: Option<&ModelSettings>,
    ) -> Result<SessionSpec, AgentError> {
        let ws_path = workspace_root.as_ref();
        let canonical_root = ws_path.canonicalize().map_err(|e| {
            AgentError::ToolError(format!("Invalid workspace path {:?}: {}", ws_path, e))
        })?;

        // Ensure workspace spec
        let ws_spec = WorkspaceSpec::new(&canonical_root).map_err(|e| {
            AgentError::ToolError(format!("Failed to create workspace spec: {}", e))
        })?;
        let ws = self
            .store
            .ensure_workspace(&ws_spec)
            .map_err(|e| AgentError::ToolError(e.to_string()))?;

        // Start with base process config
        let mut effective_config = self.config.clone();

        // Load and validate global + project config using unified HookManager logic
        let default_config_dir = HookManager::get_config_dir();
        let config_dir = self.config_dir.as_deref().unwrap_or(&default_config_dir);
        let project_config_file = canonical_root.join(".ragent/config.toml");

        let resolved_config = HookManager::load_resolved_config(config_dir, &project_config_file)?;

        if let Some(ref m) = resolved_config.model {
            effective_config.apply_model_settings(m);
        }

        // CLI model override has highest precedence
        if let Some(settings) = model_override {
            effective_config.apply_model_settings(settings);
        }

        let response_template = CreateResponseBody {
            model: Some(effective_config.model.clone()),
            temperature: effective_config.temperature,
            max_output_tokens: effective_config.max_output_tokens,
            reasoning: effective_config.reasoning.clone(),
            stream: Some(false),
            ..Default::default()
        };

        let config_rev = ConfigRevision::new(
            response_template,
            resolved_config.extensions,
            effective_config.context_summary,
        );
        let config_ref = self
            .store
            .ensure_config(&config_rev)
            .map_err(|e| AgentError::ToolError(e.to_string()))?;

        let session_id = SessionId::generate();
        let basic_prompt = match custom_prompt {
            Some(p) if !p.trim().is_empty() => p.to_string(),
            _ => build_basic_system_prompt(session_id.as_str())
                .map_err(|e| AgentError::ToolError(e.to_string()))?,
        };

        let spec = SessionSpec::new(session_id, basic_prompt, config_ref, ws.id);

        let created_spec = self
            .store
            .create_session(&spec, None, None)
            .map_err(|e| AgentError::ToolError(e.to_string()))?;

        Ok(created_spec)
    }

    pub fn get_session(
        &self,
        id: &SessionId,
    ) -> Result<Option<(SessionSpec, SessionStatus)>, AgentError> {
        let spec_opt = self
            .store
            .get_session(id)
            .map_err(|e| AgentError::ToolError(e.to_string()))?;

        let status_opt = match self.store.get_status(id) {
            Ok(Some(st)) => Some(st),
            Ok(None) if spec_opt.is_some() => Some(
                self.store
                    .rebuild_status(id)
                    .map_err(|e| AgentError::ToolError(e.to_string()))?,
            ),
            Ok(None) => None,
            Err(e) => return Err(AgentError::ToolError(e.to_string())),
        };

        match (spec_opt, status_opt) {
            (Some(spec), Some(status)) => Ok(Some((spec, status))),
            _ => Ok(None),
        }
    }

    pub fn list_sessions(&self) -> Result<Vec<(SessionSpec, SessionStatus)>, AgentError> {
        self.store
            .list_sessions()
            .map_err(|e| AgentError::ToolError(e.to_string()))
    }

    pub fn read_context(&self, id: &SessionId) -> Result<Vec<Item>, AgentError> {
        self.store
            .read_local_items(id)
            .map_err(|e| AgentError::ToolError(e.to_string()))
    }

    pub fn read_events(&self, id: &SessionId) -> Result<Vec<SessionEventEnvelope>, AgentError> {
        self.store
            .read_events(id)
            .map_err(|e| AgentError::ToolError(e.to_string()))
    }

    pub async fn run_session(
        &self,
        id: &SessionId,
        input: &str,
        cancellation: CancellationToken,
        event_callback: Option<EventCallback>,
    ) -> Result<SessionRunResult, AgentError> {
        let runner = SessionRunner::new(&self.store, &self.config);
        runner.run(id, input, cancellation, event_callback).await
    }
}
