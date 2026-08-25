pub mod manager;
pub mod runtime;
pub mod types;

pub use manager::{ExtensionConfigItem, ExtensionManager, ExtensionsConfig};
pub use runtime::WasmPlugin;
pub use types::{
    ExtensionMetadata, HookFailurePolicy, HookKind, HookRequest, HookSubscription, ToolCallRequest,
    ToolDefinition, ToolResult, ToolsListResult, HOOK_CONFIG_RESOLVE, HOOK_CONTEXT_PREPARE,
    HOOK_LOOP_AFTER, HOOK_LOOP_BEFORE, HOOK_MODEL_REQUEST_TRANSFORM, HOOK_MODEL_RESPONSE,
    HOOK_TOOLS_CALL, HOOK_TOOLS_LIST, HOOK_TOOL_RESULT_TRANSFORM,
};
