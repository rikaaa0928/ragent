use crate::config::AgentConfig;
use crate::control::lock::SessionLock;
use crate::control::EventCallback;
use crate::core::agent::AgentCore;
use crate::core::context::ContextProjection;
use crate::core::model::ModelClient;
use crate::domain::event::{SessionEvent, SessionEventEnvelope};
use crate::domain::ids::{ActivationId, SessionId, TurnId};
use crate::domain::session::{SessionPhase, SessionSpec, SessionStatus};
use crate::error::AgentError;
use crate::hooks::manager::{HookManager, PrototypePermissionPolicy};
use crate::hooks::protocol::*;
use crate::hooks::runtime::WasmPlugin;
use crate::store::sqlite::SqliteControlStore;
use openresponses_rust::{InputTokensDetails, Item, OutputTokensDetails, Tool, Usage};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct SessionRunResult {
    pub activation_id: ActivationId,
    pub final_text: String,
    pub total_usage: Option<Usage>,
    pub items: Vec<Item>,
    pub status: SessionStatus,
}

pub struct SessionRunner<'a> {
    store: &'a SqliteControlStore,
    _config: &'a AgentConfig,
    client: ModelClient,
}

impl<'a> SessionRunner<'a> {
    pub fn new(store: &'a SqliteControlStore, config: &'a AgentConfig) -> Self {
        let client = ModelClient::new(&config.base_url, &config.api_key);
        Self {
            store,
            _config: config,
            client,
        }
    }

    pub async fn run(
        &self,
        session_id: &SessionId,
        input_text: &str,
        cancellation: CancellationToken,
        event_callback: Option<EventCallback>,
    ) -> Result<SessionRunResult, AgentError> {
        // 1. Acquire single-process session lock
        let _session_lock = SessionLock::acquire(self.store.lock_dir().as_deref(), session_id)?;

        // 2. Load session spec & status
        let spec = self
            .store
            .get_session(session_id)
            .map_err(|e| AgentError::ToolError(e.to_string()))?
            .ok_or_else(|| AgentError::InvalidSessionId(session_id.to_string()))?;

        let mut status = self
            .store
            .get_or_rebuild_status(session_id)
            .map_err(|e| AgentError::ToolError(e.to_string()))?
            .ok_or_else(|| AgentError::InvalidSessionId(session_id.to_string()))?;

        // If previous process crashed leaving active activation, recover it under lock
        if status.active_activation_id.is_some() {
            self.store
                .recover_interrupted_session(
                    session_id,
                    "Process restarted while activation was active",
                )
                .map_err(|e| AgentError::ToolError(e.to_string()))?;

            status = self
                .store
                .get_or_rebuild_status(session_id)
                .map_err(|e| AgentError::ToolError(e.to_string()))?
                .ok_or_else(|| AgentError::InvalidSessionId(session_id.to_string()))?;
        }

        if status.phase != SessionPhase::Open {
            return Err(AgentError::ToolError(format!(
                "Session {} is not open (phase: {:?})",
                session_id, status.phase
            )));
        }

        let workspace = self
            .store
            .get_workspace(&spec.workspace_ref)
            .map_err(|e| AgentError::ToolError(e.to_string()))?
            .ok_or_else(|| {
                AgentError::ToolError(format!("Workspace {:?} not found", spec.workspace_ref))
            })?;

        let config_rev = self
            .store
            .get_config(&spec.default_config_ref)
            .map_err(|e| AgentError::ToolError(e.to_string()))?
            .ok_or_else(|| {
                AgentError::ToolError(format!(
                    "Config revision {:?} not found",
                    spec.default_config_ref
                ))
            })?;

        let activation_id = ActivationId::generate();
        let session_tmp_dir = PathBuf::from(format!("/tmp/ragent/{}", session_id));
        let _ = std::fs::create_dir_all(&session_tmp_dir);

        // 3. Build HookManager with PrototypePermissionPolicy
        let policy = PrototypePermissionPolicy::new(&workspace.root, &session_tmp_dir);
        let mut hook_manager = HookManager::empty().with_permission_policy(policy);

        // Load configured extensions strictly: missing extension is an explicit error
        let preopened_dirs = vec![workspace.root.clone(), session_tmp_dir.clone()];
        for ext in &config_rev.extensions {
            if ext.enabled {
                let p = Path::new(&ext.path);
                let full_path = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    workspace.root.join(p)
                };
                if !full_path.exists() {
                    return Err(AgentError::ToolError(format!(
                        "Configured extension '{}' does not exist at {:?}",
                        ext.name, full_path
                    )));
                }
                let plugin =
                    WasmPlugin::load_from_file_with_dirs(&ext.name, &full_path, &preopened_dirs)
                        .await
                        .map_err(|e| {
                            AgentError::ToolError(format!(
                                "Failed to load extension '{}' from {:?}: {}",
                                ext.name, full_path, e
                            ))
                        })?;
                hook_manager.add_plugin_with_config(plugin, ext.config.clone())?;
            }
        }

        hook_manager.validate_subscriptions()?;
        tokio::select! {
            res = hook_manager.initialize() => res?,
            _ = cancellation.cancelled() => {
                let _ = hook_manager.shutdown().await;
                return Err(AgentError::Cancelled);
            }
        };

        let run_execution = self
            .run_activation_loop(
                &spec,
                &config_rev,
                &activation_id,
                input_text,
                &mut hook_manager,
                &cancellation,
                event_callback.as_deref(),
            )
            .await;

        // Shutdown hook manager on all exits
        let _ = hook_manager.shutdown().await;

        run_execution
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_activation_loop(
        &self,
        spec: &SessionSpec,
        config_rev: &crate::domain::config::ConfigRevision,
        activation_id: &ActivationId,
        input_text: &str,
        hook_manager: &mut HookManager,
        cancellation: &CancellationToken,
        event_callback: Option<&(dyn Fn(&SessionEventEnvelope) + Send + Sync)>,
    ) -> Result<SessionRunResult, AgentError> {
        let session_id = &spec.id;

        // 1. Input prepare hook
        let input_transformed = tokio::select! {
            res = hook_manager.transform_validated(
                HOOK_INPUT_PREPARE,
                None,
                serde_json::json!({"text": input_text, "delayed": false}),
                |val| {
                    val.get("text")
                        .and_then(serde_json::Value::as_str)
                        .filter(|t| !t.is_empty())
                        .map(|_| ())
                        .ok_or_else(|| "text must be a non-empty string".into())
                },
            ) => res?,
            _ = cancellation.cancelled() => {
                return Err(AgentError::Cancelled);
            }
        };

        let prepared_text = input_transformed
            .payload
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(input_text);

        let user_item = Item::user_message(prepared_text);

        // 2. Context append prepare hook
        let append_transformed = tokio::select! {
            res = hook_manager.transform_validated(
                HOOK_CONTEXT_APPEND_PREPARE,
                None,
                serde_json::json!({
                    "reason": "input",
                    "pending": vec![user_item.clone()]
                }),
                |_| Ok(()),
            ) => res?,
            _ = cancellation.cancelled() => {
                return Err(AgentError::Cancelled);
            }
        };

        let pending_items: Vec<Item> = append_transformed
            .payload
            .get("pending")
            .and_then(|p| serde_json::from_value(p.clone()).ok())
            .unwrap_or_else(|| vec![user_item]);

        // 3. Commit Input Batch + ActivationRequested (Activation officially started)
        let (_input_batch, req_env) = self
            .store
            .commit_input(
                session_id,
                activation_id,
                &spec.default_config_ref,
                pending_items,
                None,
                None,
            )
            .map_err(|e| AgentError::ToolError(e.to_string()))?;

        if let Some(cb) = event_callback {
            cb(&req_env);
        }

        // 4. Commit ActivationStarted event
        let start_env = self
            .store
            .append_event(
                session_id,
                SessionEvent::ActivationStarted,
                Some(activation_id.clone()),
                None,
            )
            .map_err(|e| AgentError::ToolError(e.to_string()))?;

        if let Some(cb) = event_callback {
            cb(&start_env);
        }

        // Execute inner activation loop with guaranteed terminal event recording
        let mut current_turn_id: Option<TurnId> = None;
        let mut final_text = String::new();
        let mut accumulated_usage: Option<Usage> = None;

        // Model parameters are derived strictly from frozen ConfigRevision (None is explicit snapshot semantic)
        let model_name = config_rev
            .response_template
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .ok_or_else(|| {
                AgentError::ConfigError("ConfigRevision must specify a non-empty model".to_string())
            })?;
        let temperature = config_rev.response_template.temperature;
        let max_output_tokens = config_rev.response_template.max_output_tokens;
        let reasoning = config_rev.response_template.reasoning.clone();

        let mut initial_draft = AgentDraft {
            system_prompt: spec.basic_system_prompt.clone(),
            model: ModelDraft {
                name: model_name,
                temperature,
                max_output_tokens,
                reasoning,
            },
            tools: vec![],
            context: None,
        };

        let result: Result<(), AgentError> = async {
            let (draft, _) = tokio::select! {
                res = hook_manager.transform_agent_draft(HOOK_AGENT_PREPARE, None, initial_draft.clone()) => res?,
                _ = cancellation.cancelled() => {
                    return Err(AgentError::Cancelled);
                }
            };
            initial_draft = draft;

            let active_tools = initial_draft
                .tools
                .iter()
                .filter(|t| t.enabled)
                .cloned()
                .collect::<Vec<_>>();

            let request_tools = active_tools
                .iter()
                .map(|t| {
                    Tool::function(t.definition.name.clone())
                        .with_description(t.definition.description.clone())
                        .with_parameters(t.definition.parameters.clone())
                })
                .collect::<Vec<_>>();

            let mut iteration = 0;
            loop {
                if cancellation.is_cancelled() {
                    return Err(AgentError::Cancelled);
                }

                iteration += 1;
                let turn_id = TurnId::generate();
                current_turn_id = Some(turn_id.clone());

                // Emit TurnStarted
                let turn_start_env = self
                    .store
                    .append_event(
                        session_id,
                        SessionEvent::TurnStarted,
                        Some(activation_id.clone()),
                        Some(turn_id.clone()),
                    )
                    .map_err(|e| AgentError::ToolError(e.to_string()))?;
                if let Some(cb) = event_callback {
                    cb(&turn_start_env);
                }

                // Build ContextProjection from store
                let context_items = self
                    .store
                    .read_local_items(session_id)
                    .map_err(|e| AgentError::ToolError(e.to_string()))?;
                let projection = ContextProjection::new(context_items);

                let step = tokio::select! {
                    res = AgentCore::step(
                        &initial_draft.system_prompt,
                        &projection,
                        &initial_draft.model.name,
                        initial_draft.model.temperature,
                        initial_draft.model.max_output_tokens,
                        initial_draft.model.reasoning.clone(),
                        request_tools.clone(),
                        config_rev.context_summary,
                        &self.client,
                    ) => res?,
                    _ = cancellation.cancelled() => {
                        return Err(AgentError::Cancelled);
                    }
                };

                if let Some(ref usage) = step.usage {
                    accumulated_usage = Some(match accumulated_usage {
                        Some(ref acc) => add_usage(acc, usage),
                        None => usage.clone(),
                    });
                }

                if !step.text.is_empty() {
                    final_text = step.text.clone();
                }

                // Commit ModelOutput Batch + TurnCompleted
                let (_model_batch, turn_env) = self
                    .store
                    .commit_model_output(
                        session_id,
                        activation_id,
                        &turn_id,
                        step.response_id.clone(),
                        step.output_items.clone(),
                        step.usage.clone(),
                    )
                    .map_err(|e| AgentError::ToolError(e.to_string()))?;

                if let Some(cb) = event_callback {
                    cb(&turn_env);
                }

                // If model returned function calls, execute them serially
                if !step.function_calls.is_empty() {
                    for call in &step.function_calls {
                        if cancellation.is_cancelled() {
                            return Err(AgentError::Cancelled);
                        }

                        // Emit ToolCallStarted
                        let tc_start_env = self
                            .store
                            .append_event(
                                session_id,
                                SessionEvent::ToolCallStarted {
                                    call_id: call.call_id.clone(),
                                    name: call.name.clone(),
                                },
                                Some(activation_id.clone()),
                                Some(turn_id.clone()),
                            )
                            .map_err(|e| AgentError::ToolError(e.to_string()))?;
                        if let Some(cb) = event_callback {
                            cb(&tc_start_env);
                        }

                        let started_at = Instant::now();
                        let (tool_output_item, success) = tokio::select! {
                            res = self.execute_tool_call(call, &active_tools, hook_manager, iteration) => res?,
                            _ = cancellation.cancelled() => {
                                return Err(AgentError::Cancelled);
                            }
                        };

                        let duration_ms = started_at.elapsed().as_millis() as u64;

                        // Commit ToolOutput Batch + ToolCallFinished
                        let (_tool_batch, tc_fin_env) = self
                            .store
                            .commit_tool_output(
                                session_id,
                                activation_id,
                                &turn_id,
                                &call.call_id,
                                &call.name,
                                success,
                                Some(duration_ms),
                                vec![tool_output_item],
                            )
                            .map_err(|e| AgentError::ToolError(e.to_string()))?;

                        if let Some(cb) = event_callback {
                            cb(&tc_fin_env);
                        }
                    }
                } else {
                    // No function calls, ReAct activation completed!
                    let comp_env = self
                        .store
                        .append_event(
                            session_id,
                            SessionEvent::ActivationCompleted {
                                usage: accumulated_usage.clone(),
                            },
                            Some(activation_id.clone()),
                            Some(turn_id),
                        )
                        .map_err(|e| AgentError::ToolError(e.to_string()))?;
                    if let Some(cb) = event_callback {
                        cb(&comp_env);
                    }
                    break;
                }
            }

            Ok(())
        }
        .await;

        if let Err(err) = result {
            if matches!(err, AgentError::Cancelled) {
                let cancel_env_res = self.store.append_event(
                    session_id,
                    SessionEvent::ActivationCancelled,
                    Some(activation_id.clone()),
                    current_turn_id,
                );
                if let (Some(cb), Ok(cancel_env)) = (event_callback, cancel_env_res) {
                    cb(&cancel_env);
                }
            } else {
                let err_msg = err.to_string();
                let fail_env_res = self.store.append_event(
                    session_id,
                    SessionEvent::ActivationFailed {
                        error: err_msg.clone(),
                    },
                    Some(activation_id.clone()),
                    current_turn_id,
                );
                if let (Some(cb), Ok(fail_env)) = (event_callback, fail_env_res) {
                    cb(&fail_env);
                }
            }
            return Err(err);
        }

        let final_status = self
            .store
            .get_or_rebuild_status(session_id)
            .map_err(|e| AgentError::ToolError(e.to_string()))?
            .unwrap_or_else(SessionStatus::initial);

        let final_items = self
            .store
            .read_local_items(session_id)
            .map_err(|e| AgentError::ToolError(e.to_string()))?;

        Ok(SessionRunResult {
            activation_id: activation_id.clone(),
            final_text,
            total_usage: accumulated_usage,
            items: final_items,
            status: final_status,
        })
    }

    async fn execute_tool_call(
        &self,
        call: &crate::core::agent::AgentFunctionCall,
        active_tools: &[ToolEntry],
        hook_manager: &HookManager,
        iteration: usize,
    ) -> Result<(Item, bool), AgentError> {
        let tool = active_tools
            .iter()
            .find(|t| t.definition.name == call.name)
            .ok_or_else(|| AgentError::ToolNotFound(call.name.clone()))?;

        let arguments_val: serde_json::Value = serde_json::from_str(&call.arguments)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let initial_req = ToolCallRequest {
            call_id: call.call_id.clone(),
            tool_id: tool.id.clone().unwrap_or_default(),
            name: call.name.clone(),
            arguments: arguments_val,
        };

        let transformed = hook_manager
            .transform_validated(
                HOOK_TOOL_CALL_PREPARE,
                Some(iteration),
                serde_json::to_value(&initial_req)?,
                |_| Ok(()),
            )
            .await?;

        let call_req: ToolCallRequest =
            serde_json::from_value(transformed.payload).unwrap_or(initial_req);

        let owner = tool
            .owner
            .as_deref()
            .ok_or_else(|| AgentError::ToolError("Tool has no owner".into()))?;

        let action_result = hook_manager
            .action(
                HOOK_TOOLS_CALL,
                owner,
                Some(iteration),
                serde_json::to_value(call_req)?,
            )
            .await;

        let tool_result: ToolResult = match action_result {
            Ok(val) => {
                serde_json::from_value(val).unwrap_or_else(|e| ToolResult::err(e.to_string()))
            }
            Err(e) => ToolResult::err(e.to_string()),
        };

        let success = tool_result.success;

        // Tool result prepare hook
        let res_transformed = hook_manager
            .transform_validated(
                HOOK_TOOL_RESULT_PREPARE,
                Some(iteration),
                serde_json::to_value(&tool_result)?,
                |_| Ok(()),
            )
            .await?;

        let final_tool_res: ToolResult =
            serde_json::from_value(res_transformed.payload).unwrap_or(tool_result);

        let output_item = Item::FunctionCallOutput {
            id: None,
            call_id: call.call_id.clone(),
            output: final_tool_res.output.to_function_output(),
            status: None,
        };

        Ok((output_item, success))
    }
}

fn add_usage(a: &Usage, b: &Usage) -> Usage {
    Usage {
        input_tokens: a.input_tokens + b.input_tokens,
        output_tokens: a.output_tokens + b.output_tokens,
        total_tokens: a.total_tokens + b.total_tokens,
        input_tokens_details: InputTokensDetails {
            cached_tokens: a.input_tokens_details.cached_tokens
                + b.input_tokens_details.cached_tokens,
        },
        output_tokens_details: OutputTokensDetails {
            reasoning_tokens: a.output_tokens_details.reasoning_tokens
                + b.output_tokens_details.reasoning_tokens,
        },
    }
}
