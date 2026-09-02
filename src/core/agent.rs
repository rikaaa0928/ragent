use crate::config::ContextSummaryMode;
use crate::core::context::ContextProjection;
use crate::core::model::ModelClient;
use crate::error::AgentError;
use openresponses_rust::{
    CreateResponseBody, Item, MessageContent, ReasoningConfig, ResponseResource, ResponseStatus,
    Tool, ToolChoiceParam, Usage,
};

#[derive(Debug, Clone, PartialEq)]
pub struct AgentFunctionCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentCoreStepResult {
    pub output_items: Vec<Item>,
    pub function_calls: Vec<AgentFunctionCall>,
    pub response_id: Option<String>,
    pub usage: Option<Usage>,
    pub text: String,
    pub reasoning_text: String,
}

pub struct AgentCore;

impl AgentCore {
    #[allow(clippy::too_many_arguments)]
    pub async fn step(
        instructions: &str,
        projection: &ContextProjection,
        model_name: &str,
        temperature: Option<f64>,
        max_output_tokens: Option<i32>,
        reasoning: Option<ReasoningConfig>,
        tools: Vec<Tool>,
        summary_mode: ContextSummaryMode,
        client: &ModelClient,
    ) -> Result<AgentCoreStepResult, AgentError> {
        let input = projection.to_openresponses_input(summary_mode);
        let has_tools = !tools.is_empty();

        let request = CreateResponseBody {
            input: Some(input),
            model: Some(model_name.to_string()),
            instructions: Some(instructions.to_string()),
            tools: if has_tools { Some(tools) } else { None },
            tool_choice: if has_tools {
                Some(ToolChoiceParam::default())
            } else {
                None
            },
            temperature,
            max_output_tokens,
            reasoning,
            stream: Some(false),
            ..Default::default()
        };

        let response: ResponseResource = client.create_response(request).await?;

        if let Some(err) = response.error {
            return Err(AgentError::ResponseFailed(err.message));
        }
        if response.status == ResponseStatus::Failed {
            return Err(AgentError::ResponseFailed(
                "Response status is failed".into(),
            ));
        }

        let output_items = response.output;
        let mut function_calls = Vec::new();
        let mut text = String::new();
        let mut reasoning_text = String::new();

        for item in &output_items {
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
                Item::FunctionCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => {
                    function_calls.push(AgentFunctionCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                }
                _ => {}
            }
        }

        Ok(AgentCoreStepResult {
            output_items,
            function_calls,
            response_id: Some(response.id),
            usage: response.usage,
            text,
            reasoning_text,
        })
    }
}
