use serde::{Deserialize, Serialize};

pub const HOOK_CONFIG_RESOLVE: &str = "config.resolve";
pub const HOOK_CONTEXT_PREPARE: &str = "context.prepare";
pub const HOOK_LOOP_AFTER: &str = "loop.after";
pub const HOOK_LOOP_BEFORE: &str = "loop.before";
pub const HOOK_MODEL_REQUEST_TRANSFORM: &str = "model.request.transform";
pub const HOOK_MODEL_RESPONSE: &str = "model.response";
pub const HOOK_TOOL_RESULT_TRANSFORM: &str = "tool.result.transform";
pub const HOOK_TOOLS_CALL: &str = "tools.call";
pub const HOOK_TOOLS_LIST: &str = "tools.list";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    Provider,
    Transform,
    Gate,
    Action,
    Observer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HookFailurePolicy {
    #[default]
    Abort,
    Ignore,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookSubscription {
    pub hook: String,
    pub kind: HookKind,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub failure: HookFailurePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionMetadata {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub subscriptions: Vec<HookSubscription>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRequest {
    pub hook: String,
    pub version: u32,
    pub payload: serde_json::Value,
}

impl HookRequest {
    pub fn new(hook: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            hook: hook.into(),
            version: 1,
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolsListResult {
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolResult {
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: output.into(),
            error: None,
        }
    }

    pub fn err(error: impl Into<String>) -> Self {
        let error = error.into();
        Self {
            success: false,
            output: error.clone(),
            error: Some(error),
        }
    }
}
