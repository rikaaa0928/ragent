use crate::config::AgentConfig;
use crate::context::AgentContext;
use crate::error::AgentError;
use crate::event::{AgentEvent, EventHandler};
use crate::sender::AgentSender;
use crate::wasm::types::{
    HOOK_CONFIG_RESOLVE, HOOK_CONTEXT_PREPARE, HOOK_LOOP_AFTER, HOOK_LOOP_BEFORE,
    HOOK_MODEL_REQUEST_TRANSFORM, HOOK_MODEL_RESPONSE, HOOK_TOOL_RESULT_TRANSFORM,
};
use crate::wasm::ExtensionManager;
use futures::future::join_all;
use futures::StreamExt;
use openresponses_rust::{
    CreateResponseBody, FunctionOutput, Input, Item, MessageContent, StreamingClient,
    StreamingEvent, Tool, ToolChoiceParam,
};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

/// 极简本体 LLM Agent（基础 I/O + 核心 Loop，功能全靠 WASM 插件扩展）
pub struct Agent {
    config: AgentConfig,
    client: Arc<StreamingClient>,
    context: AgentContext,
    extension_manager: ExtensionManager,
    event_handler: Arc<dyn EventHandler>,
    immediate_rx: UnboundedReceiver<String>,
    delayed_rx: UnboundedReceiver<String>,
}

impl Agent {
    pub async fn new_with_manager(
        config: AgentConfig,
        manager: ExtensionManager,
    ) -> Result<(Self, AgentSender), AgentError> {
        let (immediate_tx, immediate_rx) = unbounded_channel();
        let (delayed_tx, delayed_rx) = unbounded_channel();
        manager.validate_subscriptions()?;
        manager.initialize().await?;
        let config = manager
            .transform(HOOK_CONFIG_RESOLVE, serde_json::to_value(config)?)
            .await?;
        let config: AgentConfig = serde_json::from_value(config)?;
        let client = Arc::new(StreamingClient::with_base_url(
            &config.api_key,
            &config.base_url,
        ));
        let context = AgentContext::new(None);
        let event_handler = Arc::new(crate::event::ConsoleEventHandler::new());

        let agent = Self {
            config,
            client,
            context,
            extension_manager: manager,
            event_handler,
            immediate_rx,
            delayed_rx,
        };

        let sender = AgentSender::new(immediate_tx, delayed_tx);
        Ok((agent, sender))
    }

    /// 使用 ~/.config/ragent/ 配置加载扩展
    pub async fn new(config: AgentConfig) -> Result<(Self, AgentSender), AgentError> {
        let manager = ExtensionManager::load_from_default_config().await?;
        Self::new_with_manager(config, manager).await
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
        self.extension_manager.shutdown().await
    }

    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.context.set_system_prompt(prompt);
    }

    pub fn add_user_message(&mut self, text: impl Into<String>) {
        self.context.add_user_message(text);
    }

    fn create_context_view(&self) -> serde_json::Value {
        let items = self.context.items();
        let mut recent_messages = Vec::new();

        for item in items {
            if let Item::Message { role, content, .. } = item {
                let role_str = format!("{:?}", role).to_lowercase();
                let text_content = content
                    .iter()
                    .map(|c| match c {
                        MessageContent::InputText { text } => text.clone(),
                        MessageContent::OutputText { text, .. } => text.clone(),
                        MessageContent::PlainText { text } => text.clone(),
                        MessageContent::Refusal { refusal } => refusal.clone(),
                        _ => String::new(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                recent_messages.push(serde_json::json!({
                    "role": role_str,
                    "content": text_content,
                }));
            }
        }

        serde_json::json!({
            "items_count": items.len(),
            "recent_messages": recent_messages,
        })
    }

    /// 运行 ReAct 循环主体
    pub async fn run(&mut self) -> Result<String, AgentError> {
        // 若上下文未包含任何 System Prompt，注入最简标准 prompt
        if !self.context.has_system_prompt() {
            self.context
                .set_system_prompt("你是一个高效、精准的 AI 智能体助手");
        }

        let mut iteration = 0;
        let mut last_response_text = String::new();

        loop {
            // 检查即时优先插入队列
            if let Ok(immediate_msg) = self.immediate_rx.try_recv() {
                self.event_handler.on_event(&AgentEvent::MessageReceived {
                    content: immediate_msg.clone(),
                    is_delayed: false,
                });
                self.context.add_user_message(immediate_msg);
            }

            if self.config.max_iterations > 0 && iteration >= self.config.max_iterations {
                break;
            }

            iteration += 1;
            let _iter_start = Instant::now();
            self.event_handler
                .on_event(&AgentEvent::TurnStarted { iteration });

            let context_view = self.create_context_view();
            self.extension_manager
                .observe(
                    HOOK_LOOP_BEFORE,
                    serde_json::json!({"iteration": iteration, "context": context_view}),
                )
                .await?;

            let request_items = self
                .extension_manager
                .transform(
                    HOOK_CONTEXT_PREPARE,
                    serde_json::to_value(self.context.to_items())?,
                )
                .await?;
            let request_items: Vec<Item> = serde_json::from_value(request_items)?;
            let (active_tools, active_owner_map) = self
                .extension_manager
                .resolve_tools(self.create_context_view())
                .await?;

            let mut request_tools = Vec::new();
            for t in &active_tools {
                let open_tool = Tool::function(t.name.clone())
                    .with_description(t.description.clone())
                    .with_parameters(t.parameters.clone());
                request_tools.push(open_tool);
            }

            let request = CreateResponseBody {
                input: Some(Input::Items(request_items)),
                model: Some(self.config.model.clone()),
                instructions: self.context.system_prompt().map(|s| s.to_string()),
                tools: if request_tools.is_empty() {
                    None
                } else {
                    Some(request_tools)
                },
                tool_choice: if active_tools.is_empty() {
                    None
                } else {
                    Some(ToolChoiceParam::default())
                },
                temperature: self.config.temperature,
                max_output_tokens: self.config.max_output_tokens,
                stream: Some(true),
                ..Default::default()
            };
            let request = self
                .extension_manager
                .transform(HOOK_MODEL_REQUEST_TRANSFORM, serde_json::to_value(request)?)
                .await?;
            let request: CreateResponseBody = serde_json::from_value(request)?;

            let mut stream = self
                .client
                .stream_response(request)
                .await
                .map_err(AgentError::from)?;

            let mut iter_text = String::new();
            let mut pending_tool_calls: Vec<(String, String, String)> = Vec::new(); // (id, name, args)

            while let Some(event_result) = stream.next().await {
                let event = match event_result {
                    Ok(e) => e,
                    Err(e) => {
                        let err_str = e.to_string();
                        self.event_handler
                            .on_event(&AgentEvent::Error { error: err_str });
                        return Err(AgentError::from(e));
                    }
                };

                match event {
                    StreamingEvent::OutputItemAdded {
                        item:
                            Some(Item::FunctionCall {
                                call_id,
                                name,
                                arguments,
                                ..
                            }),
                        ..
                    } => pending_tool_calls.push((call_id, name, arguments)),
                    StreamingEvent::OutputItemDone {
                        item: Some(item), ..
                    } => {
                        if let Item::FunctionCall {
                            call_id,
                            name,
                            arguments,
                            ..
                        } = &item
                        {
                            if let Some(pos) = pending_tool_calls
                                .iter()
                                .position(|(id, _, _)| *id == *call_id)
                            {
                                pending_tool_calls[pos] =
                                    (call_id.clone(), name.clone(), arguments.clone());
                            } else {
                                pending_tool_calls.push((
                                    call_id.clone(),
                                    name.clone(),
                                    arguments.clone(),
                                ));
                            }
                        }
                        self.context.add_item(item);
                    }
                    StreamingEvent::OutputTextDelta { delta, .. } => {
                        iter_text.push_str(&delta);
                        self.event_handler
                            .on_event(&AgentEvent::TextDelta { delta });
                    }
                    StreamingEvent::Error { error, .. } => {
                        let err_msg = error.message;
                        self.event_handler.on_event(&AgentEvent::Error {
                            error: err_msg.clone(),
                        });
                        return Err(AgentError::ResponseFailed(err_msg));
                    }
                    _ => {}
                }
            }

            if !iter_text.is_empty() {
                last_response_text = iter_text.clone();
            }

            self.event_handler.on_event(&AgentEvent::TurnCompleted {
                iteration,
                text: iter_text.clone(),
            });
            self.extension_manager
                .observe(
                    HOOK_MODEL_RESPONSE,
                    serde_json::json!({"iteration": iteration, "text": iter_text}),
                )
                .await?;

            // 执行这一轮收集到的所有 WASM 工具调用
            if !pending_tool_calls.is_empty() {
                let mut tool_futures = Vec::new();

                for (call_id, name, args) in pending_tool_calls {
                    let event_handler = Arc::clone(&self.event_handler);
                    let ext_manager = &self.extension_manager;
                    let owner_map = &active_owner_map;

                    tool_futures.push(async move {
                        event_handler.on_event(&AgentEvent::ToolCallStarted {
                            call_id: call_id.clone(),
                            tool_name: name.clone(),
                            arguments: args.clone(),
                        });

                        let t_start = Instant::now();
                        let result = match serde_json::from_str(&args) {
                            Ok(arguments) => {
                                ext_manager.execute_tool(owner_map, &name, arguments).await
                            }
                            Err(error) => Ok(crate::wasm::ToolResult::err(format!(
                                "invalid tool arguments: {error}"
                            ))),
                        };

                        match result {
                            Ok(res) => {
                                let transformed = ext_manager
                                    .transform(
                                        HOOK_TOOL_RESULT_TRANSFORM,
                                        serde_json::to_value(&res).unwrap_or_default(),
                                    )
                                    .await;
                                let res = transformed
                                    .and_then(|value| {
                                        serde_json::from_value(value).map_err(AgentError::JsonError)
                                    })
                                    .unwrap_or_else(|error| {
                                        crate::wasm::ToolResult::err(error.to_string())
                                    });
                                event_handler.on_event(&AgentEvent::ToolCallFinished {
                                    call_id: call_id.clone(),
                                    tool_name: name.clone(),
                                    output: res.output.clone(),
                                    is_error: !res.success,
                                    duration_ms: t_start.elapsed().as_millis(),
                                });

                                (call_id, res.output)
                            }
                            Err(e) => {
                                let err_output = format!("工具执行失败: {}", e);
                                event_handler.on_event(&AgentEvent::ToolCallFinished {
                                    call_id: call_id.clone(),
                                    tool_name: name.clone(),
                                    output: err_output.clone(),
                                    is_error: true,
                                    duration_ms: t_start.elapsed().as_millis(),
                                });

                                (call_id, err_output)
                            }
                        }
                    });
                }

                let executed_results = join_all(tool_futures).await;

                for (call_id, output) in executed_results {
                    let function_output = Item::FunctionCallOutput {
                        id: None,
                        call_id,
                        output: FunctionOutput::Text(output),
                        status: None,
                    };
                    self.context.add_item(function_output);
                }

                self.event_handler
                    .on_event(&AgentEvent::RoundCompleted { iteration });
                self.extension_manager
                    .observe(
                        HOOK_LOOP_AFTER,
                        serde_json::json!({"iteration": iteration, "called_tools": true}),
                    )
                    .await?;
                // 继续下一轮 ReAct 迭代
                continue;
            }

            self.event_handler
                .on_event(&AgentEvent::RoundCompleted { iteration });

            self.extension_manager
                .observe(
                    HOOK_LOOP_AFTER,
                    serde_json::json!({"iteration": iteration, "called_tools": false}),
                )
                .await?;

            // 无工具调用，检查延迟输入队列
            if let Ok(delayed_msg) = self.delayed_rx.try_recv() {
                self.event_handler.on_event(&AgentEvent::MessageReceived {
                    content: delayed_msg.clone(),
                    is_delayed: true,
                });
                self.context.add_user_message(delayed_msg);
                continue;
            }

            // 迭代结束
            break;
        }

        self.event_handler.on_event(&AgentEvent::AgentFinished);

        Ok(last_response_text)
    }
}
