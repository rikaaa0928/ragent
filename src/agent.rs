use crate::config::AgentConfig;
use crate::context::AgentContext;
use crate::error::AgentError;
use crate::event::{AgentEvent, EventHandler, TokenUsage};
use crate::sender::AgentSender;
use crate::wasm::types::*;
use crate::wasm::ExtensionManager;
use openresponses_rust::{
    Client, CreateResponseBody, FunctionOutput, Input, Item, MessageContent, ResponseStatus, Tool,
    ToolChoiceParam,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio_util::sync::CancellationToken;

const DEFAULT_SYSTEM_PROMPT: &str = "你是一个高效、精准、善于深度思考的 AI 智能体助手";

/// 只负责模型 I/O、上下文提交与 ReAct loop；其余行为由 WASM hook 提供。
pub struct Agent {
    config: AgentConfig,
    client: Arc<Client>,
    context: AgentContext,
    base_draft: AgentDraft,
    extension_manager: ExtensionManager,
    event_handler: Arc<dyn EventHandler>,
    pending_inputs: Vec<String>,
    immediate_rx: UnboundedReceiver<String>,
    delayed_rx: UnboundedReceiver<String>,
    cancellation: CancellationToken,
}

impl Agent {
    pub async fn new_with_manager(
        mut config: AgentConfig,
        manager: ExtensionManager,
    ) -> Result<(Self, AgentSender), AgentError> {
        let (immediate_tx, immediate_rx) = unbounded_channel();
        let (delayed_tx, delayed_rx) = unbounded_channel();
        let cancellation = CancellationToken::new();
        manager.validate_subscriptions()?;
        manager.initialize().await?;

        if let Some(model_settings) = manager.model_settings() {
            config.apply_model_settings(model_settings);
        }
        if config.model.trim().is_empty() {
            return Err(AgentError::ToolError(
                "model name is not configured. Please specify [model] name in config.toml".into(),
            ));
        }

        let draft = AgentDraft {
            system_prompt: DEFAULT_SYSTEM_PROMPT.into(),
            model: ModelDraft {
                name: config.model.clone(),
                temperature: config.temperature,
                max_output_tokens: config.max_output_tokens,
                reasoning: config.reasoning.clone(),
            },
            tools: vec![],
            context: None,
        };
        let (base_draft, control) = manager
            .transform_agent_draft(HOOK_AGENT_PREPARE, None, draft)
            .await?;
        if control != FlowControl::Continue {
            return Err(AgentError::HookRejected {
                hook: HOOK_AGENT_PREPARE.into(),
                reason: "agent initialization was skipped or stopped".into(),
            });
        }
        config.model = base_draft.model.name.clone();
        config.temperature = base_draft.model.temperature;
        config.max_output_tokens = base_draft.model.max_output_tokens;
        config.reasoning = base_draft.model.reasoning.clone();

        let client = Arc::new(Client::with_base_url(&config.api_key, &config.base_url));
        let context = AgentContext::new(Some(base_draft.system_prompt.clone()));
        let agent = Self {
            config,
            client,
            context,
            base_draft,
            extension_manager: manager,
            event_handler: Arc::new(crate::event::ConsoleEventHandler::new()),
            pending_inputs: vec![],
            immediate_rx,
            delayed_rx,
            cancellation: cancellation.clone(),
        };
        Ok((
            agent,
            AgentSender::with_cancellation(immediate_tx, delayed_tx, cancellation),
        ))
    }

    pub async fn new(config: AgentConfig) -> Result<(Self, AgentSender), AgentError> {
        Self::new_with_manager(config, ExtensionManager::load_from_default_config().await?).await
    }

    pub fn context(&self) -> &AgentContext {
        &self.context
    }
    pub fn context_mut(&mut self) -> &mut AgentContext {
        &mut self.context
    }
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }
    pub fn config_mut(&mut self) -> &mut AgentConfig {
        &mut self.config
    }
    pub fn event_handler(&self) -> &Arc<dyn EventHandler> {
        &self.event_handler
    }
    pub fn set_event_handler(&mut self, handler: Arc<dyn EventHandler>) {
        self.event_handler = handler;
    }
    pub fn extension_manager(&self) -> &ExtensionManager {
        &self.extension_manager
    }

    pub async fn shutdown(&self) -> Result<(), AgentError> {
        let observed = self
            .extension_manager
            .observe(HOOK_AGENT_SHUTDOWN, None, serde_json::json!({}))
            .await;
        let shutdown = self.extension_manager.shutdown().await;
        observed?;
        shutdown
    }

    pub fn add_user_message(&mut self, text: impl Into<String>) {
        self.pending_inputs.push(text.into());
    }

    fn context_view(&self) -> Value {
        let recent_messages = self.context.items().iter().filter_map(|item| {
            if let Item::Message { role, content, .. } = item {
                let text = content.iter().filter_map(|part| match part {
                    MessageContent::InputText { text } | MessageContent::OutputText { text, .. }
                    | MessageContent::PlainText { text } => Some(text.as_str()),
                    MessageContent::Refusal { refusal } => Some(refusal.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join("\n");
                Some(serde_json::json!({"role": format!("{role:?}").to_lowercase(), "content": text}))
            } else { None }
        }).collect::<Vec<_>>();
        serde_json::json!({"items_count": self.context.items().len(), "recent_messages": recent_messages})
    }

    async fn accept_input(
        &mut self,
        text: String,
        delayed: bool,
    ) -> Result<FlowControl, AgentError> {
        self.event_handler.on_event(&AgentEvent::MessageReceived {
            content: text.clone(),
            is_delayed: delayed,
        });
        let transformed = self
            .extension_manager
            .transform_validated(
                HOOK_INPUT_PREPARE,
                None,
                serde_json::json!({"text": text, "delayed": delayed}),
                |value| {
                    value
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(|_| ())
                        .ok_or_else(|| "text must be a non-empty string".into())
                },
            )
            .await?;
        if transformed.control == FlowControl::Stop {
            return Ok(FlowControl::Stop);
        }
        if transformed.control == FlowControl::Skip {
            return Ok(FlowControl::Skip);
        }
        let text = transformed
            .payload
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AgentError::ToolError("input.prepare must return a string field 'text'".into())
            })?;
        self.commit_context("input", vec![Item::user_message(text)], None)
            .await
    }

    async fn commit_context(
        &mut self,
        reason: &str,
        pending: Vec<Item>,
        iteration: Option<usize>,
    ) -> Result<FlowControl, AgentError> {
        let mut next = self.context.to_items();
        next.extend(pending.clone());
        let result = self
            .extension_manager
            .transform_validated(
                HOOK_CONTEXT_COMMIT,
                iteration,
                serde_json::json!({
                    "reason": reason, "current": self.context.to_items(), "pending": pending, "next": next
                }),
                validate_context_commit,
            )
            .await?;
        if result.control != FlowControl::Continue {
            return Ok(result.control);
        }
        let next: Vec<Item> =
            serde_json::from_value(result.payload.get("next").cloned().ok_or_else(|| {
                AgentError::ToolError("context.commit must return 'next'".into())
            })?)?;
        self.context.replace_items(next);
        Ok(FlowControl::Continue)
    }

    pub async fn run(&mut self) -> Result<String, AgentError> {
        let cancellation = self.cancellation.clone();
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Ok((String::new(), TokenUsage::default())),
            result = self.run_inner() => result,
        };
        if let Ok((_, total_usage)) = &result {
            let usage_opt = if total_usage.total_tokens > 0 {
                Some(total_usage.clone())
            } else {
                None
            };
            self.event_handler.on_event(&AgentEvent::AgentFinished {
                total_usage: usage_opt,
            });
        }
        if let Err(error) = &result {
            let _ = self
                .extension_manager
                .observe(
                    HOOK_AGENT_ERROR,
                    None,
                    serde_json::json!({"error": error.to_string()}),
                )
                .await;
        }
        result.map(|(text, _)| text)
    }

    async fn run_inner(&mut self) -> Result<(String, TokenUsage), AgentError> {
        for input in std::mem::take(&mut self.pending_inputs) {
            if self.accept_input(input, false).await? == FlowControl::Stop {
                return Ok((String::new(), TokenUsage::default()));
            }
        }
        let mut iteration = 0;
        let mut last_response_text = String::new();
        let mut total_usage = TokenUsage::default();

        loop {
            while let Ok(input) = self.immediate_rx.try_recv() {
                if self.accept_input(input, false).await? == FlowControl::Stop {
                    return Ok((last_response_text, total_usage));
                }
            }
            if self.config.max_iterations > 0 && iteration >= self.config.max_iterations {
                break;
            }
            iteration += 1;
            self.event_handler
                .on_event(&AgentEvent::TurnStarted { iteration });

            let mut draft = self.base_draft.clone();
            draft.context = Some(serde_json::json!({
                "items": self.context.to_items(),
                "view": self.context_view()
            }));
            let (draft, turn_control) = self
                .extension_manager
                .transform_agent_draft(HOOK_TURN_PREPARE, Some(iteration), draft)
                .await?;
            if turn_control == FlowControl::Stop {
                break;
            }
            if turn_control == FlowControl::Skip {
                continue;
            }

            let active_tools = draft
                .tools
                .iter()
                .filter(|tool| tool.enabled)
                .cloned()
                .collect::<Vec<_>>();
            let request_tools = active_tools
                .iter()
                .map(|tool| {
                    Tool::function(tool.definition.name.clone())
                        .with_description(tool.definition.description.clone())
                        .with_parameters(tool.definition.parameters.clone())
                })
                .collect::<Vec<_>>();
            let request = CreateResponseBody {
                input: Some(Input::Items(self.context.to_items())),
                model: Some(draft.model.name.clone()),
                instructions: Some(draft.system_prompt.clone()),
                tools: (!request_tools.is_empty()).then_some(request_tools),
                tool_choice: (!active_tools.is_empty()).then_some(ToolChoiceParam::default()),
                temperature: draft.model.temperature,
                max_output_tokens: draft.model.max_output_tokens,
                reasoning: draft.model.reasoning.clone(),
                stream: Some(false),
                ..Default::default()
            };
            let transformed = self
                .extension_manager
                .transform_validated(
                    HOOK_MODEL_REQUEST_PREPARE,
                    Some(iteration),
                    serde_json::to_value(request)?,
                    validate_model_request,
                )
                .await?;
            if transformed.control == FlowControl::Stop {
                break;
            }
            if transformed.control == FlowControl::Skip {
                continue;
            }
            let request: CreateResponseBody = serde_json::from_value(transformed.payload)?;

            let response = self
                .client
                .create_response(request)
                .await
                .map_err(AgentError::from)?;

            self.extension_manager
                .observe(
                    HOOK_MODEL_RESPONSE_OBSERVE,
                    Some(iteration),
                    serde_json::to_value(&response)?,
                )
                .await?;

            if let Some(err) = response.error {
                return Err(AgentError::ResponseFailed(err.message));
            }
            if response.status == ResponseStatus::Failed {
                return Err(AgentError::ResponseFailed(
                    "response status is failed".into(),
                ));
            }

            let turn_usage = response.usage.as_ref().map(TokenUsage::from);
            if let Some(usage) = &turn_usage {
                total_usage += usage;
            }

            let response_items = response.output;
            let mut text = String::new();
            let mut reasoning_text = String::new();
            for item in &response_items {
                match item {
                    Item::Message { content, .. } => {
                        for part in content {
                            match part {
                                MessageContent::OutputText {
                                    text: part_text, ..
                                }
                                | MessageContent::PlainText { text: part_text } => {
                                    text.push_str(part_text);
                                }
                                MessageContent::Refusal { refusal } => {
                                    text.push_str(refusal);
                                }
                                _ => {}
                            }
                        }
                    }
                    Item::Reasoning {
                        summary, content, ..
                    } => {
                        for part in summary {
                            match part {
                                MessageContent::SummaryText { text: part_text }
                                | MessageContent::OutputText {
                                    text: part_text, ..
                                }
                                | MessageContent::PlainText { text: part_text } => {
                                    reasoning_text.push_str(part_text);
                                }
                                _ => {}
                            }
                        }
                        if reasoning_text.is_empty() {
                            if let Some(content_parts) = content {
                                for part in content_parts {
                                    match part {
                                        MessageContent::SummaryText { text: part_text }
                                        | MessageContent::OutputText {
                                            text: part_text, ..
                                        }
                                        | MessageContent::PlainText { text: part_text } => {
                                            reasoning_text.push_str(part_text);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            let transformed = self
                .extension_manager
                .transform_validated(
                    HOOK_MODEL_RESPONSE_PREPARE,
                    Some(iteration),
                    serde_json::json!({"text": text, "items": response_items}),
                    validate_model_response,
                )
                .await?;
            if transformed.control == FlowControl::Stop {
                break;
            }
            if transformed.control == FlowControl::Skip {
                continue;
            }
            let response_text = transformed
                .payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let response_items: Vec<Item> =
                serde_json::from_value(transformed.payload.get("items").cloned().ok_or_else(
                    || AgentError::ToolError("model.response.prepare must return 'items'".into()),
                )?)?;
            if !response_text.is_empty() {
                last_response_text = response_text.clone();
            }
            let response_commit = self
                .commit_context("model_response", response_items.clone(), Some(iteration))
                .await?;
            match response_commit {
                FlowControl::Stop => break,
                FlowControl::Skip => continue,
                FlowControl::Continue => {}
            }
            self.event_handler.on_event(&AgentEvent::TurnCompleted {
                iteration,
                text: response_text.clone(),
                reasoning: (!reasoning_text.is_empty()).then_some(reasoning_text),
                usage: turn_usage.clone(),
            });

            let calls = response_items
                .into_iter()
                .filter_map(|item| match item {
                    Item::FunctionCall {
                        call_id,
                        name,
                        arguments,
                        ..
                    } => Some((call_id, name, arguments)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let called_tools = !calls.is_empty();
            if called_tools {
                let mut outputs = Vec::new();
                let mut stop = false;
                for (call_id, name, arguments) in calls {
                    let (output, control) = self
                        .execute_tool_call(iteration, &active_tools, call_id, name, arguments)
                        .await?;
                    if let Some(output) = output {
                        outputs.push(output);
                    }
                    if control == FlowControl::Stop {
                        stop = true;
                        break;
                    }
                }
                if !outputs.is_empty() {
                    stop = self
                        .commit_context("tool_results", outputs, Some(iteration))
                        .await?
                        != FlowControl::Continue;
                }
                if stop {
                    break;
                }
            }

            self.event_handler.on_event(&AgentEvent::RoundCompleted {
                iteration,
                usage: turn_usage,
                total_usage: (total_usage.total_tokens > 0).then(|| total_usage.clone()),
            });
            let complete = self
                .extension_manager
                .transform_validated(
                    HOOK_TURN_COMPLETE,
                    Some(iteration),
                    serde_json::json!({"iteration":iteration, "called_tools":called_tools, "continue_loop":called_tools}),
                    |value| {
                        value
                            .get("continue_loop")
                            .and_then(Value::as_bool)
                            .map(|_| ())
                            .ok_or_else(|| "continue_loop must be boolean".into())
                    },
                )
                .await?;
            if complete.control == FlowControl::Stop {
                break;
            }
            let mut continue_loop = complete
                .payload
                .get("continue_loop")
                .and_then(Value::as_bool)
                .unwrap_or(called_tools);
            if !called_tools {
                if let Ok(input) = self.delayed_rx.try_recv() {
                    if self.accept_input(input, true).await? == FlowControl::Stop {
                        break;
                    }
                    continue_loop = true;
                }
            }
            if !continue_loop {
                break;
            }
        }
        Ok((last_response_text, total_usage))
    }

    async fn execute_tool_call(
        &self,
        iteration: usize,
        tools: &[ToolEntry],
        call_id: String,
        name: String,
        args: String,
    ) -> Result<(Option<Item>, FlowControl), AgentError> {
        let started = Instant::now();
        self.event_handler.on_event(&AgentEvent::ToolCallStarted {
            call_id: call_id.clone(),
            tool_name: name.clone(),
            arguments: args.clone(),
        });
        let tool = tools
            .iter()
            .find(|tool| tool.definition.name == name)
            .ok_or_else(|| AgentError::ToolNotFound(name.clone()))?;
        let arguments = serde_json::from_str(&args)
            .map_err(|e| AgentError::ToolError(format!("invalid tool arguments: {e}")))?;
        let initial = ToolCallRequest {
            call_id: call_id.clone(),
            tool_id: tool.id.clone().unwrap_or_default(),
            name: name.clone(),
            arguments,
        };
        let transformed = self
            .extension_manager
            .transform_validated(
                HOOK_TOOL_CALL_PREPARE,
                Some(iteration),
                serde_json::to_value(&initial)?,
                |value| {
                    serde_json::from_value::<ToolCallRequest>(value.clone())
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                },
            )
            .await?;
        match transformed.control {
            FlowControl::Stop => return Ok((None, FlowControl::Stop)),
            FlowControl::Skip => {
                return Ok((
                    Some(Item::FunctionCallOutput {
                        id: None,
                        call_id,
                        output: FunctionOutput::Text("tool call skipped by extension".into()),
                        status: None,
                    }),
                    FlowControl::Continue,
                ));
            }
            FlowControl::Continue => {}
        }
        let mut call: ToolCallRequest = serde_json::from_value(transformed.payload)?;
        if call.call_id != initial.call_id || call.tool_id != initial.tool_id {
            return Err(AgentError::ToolError(
                "tool.call.prepare cannot change call_id or tool_id".into(),
            ));
        }
        call.name = tool.definition.name.clone();
        let owner = tool
            .owner
            .as_deref()
            .ok_or_else(|| AgentError::ToolError("tool has no owner".into()))?;
        let value = self
            .extension_manager
            .action(
                HOOK_TOOLS_CALL,
                owner,
                Some(iteration),
                serde_json::to_value(&call)?,
            )
            .await?;
        if transformed.control == FlowControl::Stop {
            return Ok((None, FlowControl::Stop));
        }
        let result: ToolResult = serde_json::from_value(value)?;
        let transformed = self
            .extension_manager
            .transform_validated(
                HOOK_TOOL_RESULT_PREPARE,
                Some(iteration),
                serde_json::json!({
                    "call": call, "result": result
                }),
                |value| {
                    value
                        .get("result")
                        .cloned()
                        .ok_or_else(|| "missing result".into())
                        .and_then(|result| {
                            serde_json::from_value::<ToolResult>(result)
                                .map(|_| ())
                                .map_err(|error| error.to_string())
                        })
                },
            )
            .await?;
        let result: ToolResult =
            serde_json::from_value(transformed.payload.get("result").cloned().ok_or_else(
                || AgentError::ToolError("tool.result.prepare must return 'result'".into()),
            )?)?;
        self.event_handler.on_event(&AgentEvent::ToolCallFinished {
            call_id: call_id.clone(),
            tool_name: name,
            output: result.output.to_display_string(),
            is_error: !result.success,
            duration_ms: started.elapsed().as_millis(),
        });
        Ok((
            Some(Item::FunctionCallOutput {
                id: None,
                call_id,
                output: result.output.to_function_output(),
                status: None,
            }),
            FlowControl::Continue,
        ))
    }
}

fn validate_model_request(value: &Value) -> Result<(), String> {
    let request: CreateResponseBody =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    if request.model.as_deref().is_none_or(str::is_empty) {
        return Err("model must not be empty".into());
    }
    if request
        .temperature
        .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
    {
        return Err("temperature must be between 0 and 2".into());
    }
    if request.max_output_tokens.is_some_and(|value| value <= 0) {
        return Err("max_output_tokens must be positive".into());
    }
    if request.stream == Some(true) {
        return Err("stream must be false for non-streaming model I/O".into());
    }
    if request.background == Some(true) {
        return Err("background responses are not supported by the Agent loop".into());
    }
    Ok(())
}

fn validate_model_response(value: &Value) -> Result<(), String> {
    if !value.get("text").is_some_and(Value::is_string) {
        return Err("text must be a string".into());
    }
    let items = value
        .get("items")
        .cloned()
        .ok_or_else(|| "missing items".to_string())?;
    serde_json::from_value::<Vec<Item>>(items)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn validate_context_commit(value: &Value) -> Result<(), String> {
    let items: Vec<Item> = serde_json::from_value(
        value
            .get("next")
            .cloned()
            .ok_or_else(|| "missing next".to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let mut calls = std::collections::HashSet::new();
    for item in items {
        match item {
            Item::Message { role, .. } if format!("{role:?}").eq_ignore_ascii_case("system") => {
                return Err("system messages must stay outside context items".into());
            }
            Item::FunctionCall { call_id, .. } if !calls.insert(call_id.clone()) => {
                return Err("duplicate function call_id".into());
            }
            Item::FunctionCallOutput { call_id, .. } if !calls.contains(&call_id) => {
                return Err(format!(
                    "function output references unknown call_id '{call_id}'"
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod hook_validation_tests {
    use super::*;

    #[test]
    fn rejects_invalid_model_parameters() {
        let request = CreateResponseBody {
            model: Some("test".into()),
            temperature: Some(3.0),
            ..Default::default()
        };
        assert!(validate_model_request(&serde_json::to_value(request).unwrap()).is_err());
    }

    #[test]
    fn rejects_streaming_and_background_model_requests() {
        for request in [
            CreateResponseBody {
                model: Some("test".into()),
                stream: Some(true),
                ..Default::default()
            },
            CreateResponseBody {
                model: Some("test".into()),
                background: Some(true),
                ..Default::default()
            },
        ] {
            assert!(validate_model_request(&serde_json::to_value(request).unwrap()).is_err());
        }
    }

    #[test]
    fn context_commit_rejects_unknown_tool_output() {
        let output = Item::FunctionCallOutput {
            id: None,
            call_id: "missing".into(),
            output: FunctionOutput::Text("result".into()),
            status: None,
        };
        assert!(validate_context_commit(&serde_json::json!({"next": [output]})).is_err());
    }

    #[test]
    fn context_commit_accepts_matching_tool_output() {
        let call = Item::FunctionCall {
            id: None,
            call_id: "call-1".into(),
            name: "shell".into(),
            arguments: "{}".into(),
            status: None,
        };
        let output = Item::FunctionCallOutput {
            id: None,
            call_id: "call-1".into(),
            output: FunctionOutput::Text("result".into()),
            status: None,
        };
        assert!(validate_context_commit(&serde_json::json!({"next": [call, output]})).is_ok());
    }
}
