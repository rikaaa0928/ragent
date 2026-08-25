use crate::error::AgentError;
use crate::wasm::runtime::WasmPlugin;
use crate::wasm::types::{
    HookFailurePolicy, HookKind, HookRequest, HookSubscription, ToolCallRequest, ToolDefinition,
    ToolResult, ToolsListResult, HOOK_TOOLS_CALL, HOOK_TOOLS_LIST,
};
use directories::BaseDirs;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionConfigItem {
    pub name: String,
    pub path: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtensionsConfig {
    #[serde(default)]
    pub extensions: Vec<ExtensionConfigItem>,
}

#[derive(Clone)]
struct Subscriber {
    plugin: Arc<WasmPlugin>,
    subscription: HookSubscription,
}

pub struct ExtensionManager {
    plugins: Vec<Arc<WasmPlugin>>,
    plugin_config: HashMap<String, Value>,
    config_dir: PathBuf,
}

impl ExtensionManager {
    pub fn get_config_dir() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.trim().is_empty() {
                return PathBuf::from(xdg).join("ragent");
            }
        }
        if let Some(home) = dirs::home_dir() {
            home.join(".config").join("ragent")
        } else if let Some(base_dirs) = BaseDirs::new() {
            base_dirs.config_dir().join("ragent")
        } else {
            PathBuf::from(".config/ragent")
        }
    }

    pub async fn load_from_default_config() -> Result<Self, AgentError> {
        Self::load_from_dir(&Self::get_config_dir()).await
    }

    pub async fn load_from_dir(config_dir: &Path) -> Result<Self, AgentError> {
        if !config_dir.exists() {
            fs::create_dir_all(config_dir.join("extensions")).map_err(|error| {
                AgentError::ToolError(format!("failed to create extension config dir: {error}"))
            })?;
        }

        let config_file = config_dir.join("config.toml");
        let config = if config_file.exists() {
            let content = fs::read_to_string(&config_file).map_err(|error| {
                AgentError::ToolError(format!("failed to read {config_file:?}: {error}"))
            })?;
            toml::from_str::<ExtensionsConfig>(&content).map_err(|error| {
                AgentError::ToolError(format!("failed to parse {config_file:?}: {error}"))
            })?
        } else {
            ExtensionsConfig::default()
        };

        let mut manager = Self {
            plugins: Vec::new(),
            plugin_config: HashMap::new(),
            config_dir: config_dir.to_path_buf(),
        };
        for item in config.extensions.into_iter().filter(|item| item.enabled) {
            let path = if Path::new(&item.path).is_absolute() {
                PathBuf::from(&item.path)
            } else {
                config_dir.join(&item.path)
            };
            if !path.exists() {
                return Err(AgentError::ToolError(format!(
                    "configured extension '{}' does not exist at {path:?}",
                    item.name
                )));
            }
            let plugin = WasmPlugin::load_from_file(&item.name, &path).await?;
            manager.add_plugin_with_config(plugin, item.config)?;
        }
        Ok(manager)
    }

    pub fn empty() -> Self {
        Self {
            plugins: Vec::new(),
            plugin_config: HashMap::new(),
            config_dir: Self::get_config_dir(),
        }
    }

    pub fn add_plugin(&mut self, plugin: WasmPlugin) -> Result<(), AgentError> {
        self.add_plugin_with_config(plugin, Value::Null)
    }

    pub fn add_plugin_with_config(
        &mut self,
        plugin: WasmPlugin,
        config: Value,
    ) -> Result<(), AgentError> {
        let id = plugin.metadata().id.clone();
        if self.plugins.iter().any(|loaded| loaded.metadata().id == id) {
            return Err(AgentError::ToolError(format!(
                "duplicate extension id '{id}'"
            )));
        }
        self.plugin_config.insert(id, config);
        self.plugins.push(Arc::new(plugin));
        Ok(())
    }

    pub fn plugins(&self) -> &[Arc<WasmPlugin>] {
        &self.plugins
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub async fn initialize(&self) -> Result<(), AgentError> {
        for plugin in &self.plugins {
            let config = self
                .plugin_config
                .get(&plugin.metadata().id)
                .unwrap_or(&Value::Null);
            plugin.initialize(config).await?;
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), AgentError> {
        for result in join_all(self.plugins.iter().map(|plugin| plugin.shutdown())).await {
            result?;
        }
        Ok(())
    }

    pub async fn provide(&self, hook: &str, payload: Value) -> Result<Vec<Value>, AgentError> {
        let mut values = Vec::new();
        for subscriber in self.subscribers(hook, HookKind::Provider) {
            match subscriber
                .plugin
                .invoke(&HookRequest::new(hook, payload.clone()))
                .await
            {
                Ok(value) => values.push(value),
                Err(_) if subscriber.subscription.failure == HookFailurePolicy::Ignore => {}
                Err(error) => return Err(error),
            }
        }
        Ok(values)
    }

    pub async fn transform(&self, hook: &str, mut payload: Value) -> Result<Value, AgentError> {
        for subscriber in self.subscribers(hook, HookKind::Transform) {
            match subscriber
                .plugin
                .invoke(&HookRequest::new(hook, payload.clone()))
                .await
            {
                Ok(value) => payload = value,
                Err(_) if subscriber.subscription.failure == HookFailurePolicy::Ignore => {}
                Err(error) => return Err(error),
            }
        }
        Ok(payload)
    }

    pub async fn gate(&self, hook: &str, payload: Value) -> Result<(), AgentError> {
        for subscriber in self.subscribers(hook, HookKind::Gate) {
            match subscriber
                .plugin
                .invoke(&HookRequest::new(hook, payload.clone()))
                .await
            {
                Ok(Value::Bool(true)) => {}
                Ok(Value::Bool(false)) => {
                    return Err(AgentError::HookRejected {
                        hook: hook.to_string(),
                        reason: subscriber.plugin.metadata().id.clone(),
                    });
                }
                Ok(value) => {
                    return Err(AgentError::ToolError(format!(
                        "gate hook '{hook}' returned non-boolean value: {value}"
                    )));
                }
                Err(_) if subscriber.subscription.failure == HookFailurePolicy::Ignore => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub async fn observe(&self, hook: &str, payload: Value) -> Result<(), AgentError> {
        for subscriber in self.subscribers(hook, HookKind::Observer) {
            match subscriber
                .plugin
                .invoke(&HookRequest::new(hook, payload.clone()))
                .await
            {
                Ok(_) => {}
                Err(_) if subscriber.subscription.failure == HookFailurePolicy::Ignore => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub async fn resolve_tools(
        &self,
        context: Value,
    ) -> Result<(Vec<ToolDefinition>, HashMap<String, Arc<WasmPlugin>>), AgentError> {
        let mut tools = Vec::new();
        let mut owners = HashMap::new();
        for subscriber in self.subscribers(HOOK_TOOLS_LIST, HookKind::Provider) {
            let value = match subscriber
                .plugin
                .invoke(&HookRequest::new(HOOK_TOOLS_LIST, context.clone()))
                .await
            {
                Ok(value) => value,
                Err(_) if subscriber.subscription.failure == HookFailurePolicy::Ignore => {
                    continue;
                }
                Err(error) => return Err(error),
            };
            let provided: ToolsListResult = serde_json::from_value(value)?;
            for tool in provided.tools {
                if owners.contains_key(&tool.name) {
                    return Err(AgentError::ToolError(format!(
                        "duplicate tool name '{}'",
                        tool.name
                    )));
                }
                owners.insert(tool.name.clone(), Arc::clone(&subscriber.plugin));
                tools.push(tool);
            }
        }
        Ok((tools, owners))
    }

    pub async fn execute_tool(
        &self,
        owners: &HashMap<String, Arc<WasmPlugin>>,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, AgentError> {
        let plugin = owners
            .get(name)
            .ok_or_else(|| AgentError::ToolNotFound(name.to_string()))?;
        if !plugin.metadata().subscriptions.iter().any(|subscription| {
            subscription.hook == HOOK_TOOLS_CALL && subscription.kind == HookKind::Action
        }) {
            return Err(AgentError::ToolError(format!(
                "extension '{}' provides tool '{name}' but does not subscribe to tools.call",
                plugin.metadata().id
            )));
        }
        let value = plugin
            .invoke(&HookRequest::new(
                HOOK_TOOLS_CALL,
                serde_json::to_value(ToolCallRequest {
                    name: name.to_string(),
                    arguments,
                })?,
            ))
            .await?;
        serde_json::from_value(value).map_err(AgentError::JsonError)
    }

    fn subscribers(&self, hook: &str, kind: HookKind) -> Vec<Subscriber> {
        let mut subscribers = self
            .plugins
            .iter()
            .flat_map(|plugin| {
                plugin
                    .metadata()
                    .subscriptions
                    .iter()
                    .filter(move |subscription| {
                        subscription.hook == hook && subscription.kind == kind
                    })
                    .map(move |subscription| Subscriber {
                        plugin: Arc::clone(plugin),
                        subscription: subscription.clone(),
                    })
            })
            .collect::<Vec<_>>();
        subscribers.sort_by_key(|subscriber| subscriber.subscription.priority);
        subscribers
    }

    pub fn validate_subscriptions(&self) -> Result<(), AgentError> {
        let mut seen = HashSet::new();
        for plugin in &self.plugins {
            for subscription in &plugin.metadata().subscriptions {
                let key = (
                    plugin.metadata().id.clone(),
                    subscription.hook.clone(),
                    subscription.kind,
                );
                if !seen.insert(key) {
                    return Err(AgentError::ToolError(format!(
                        "extension '{}' declares hook '{}' more than once",
                        plugin.metadata().id,
                        subscription.hook
                    )));
                }
            }
        }
        Ok(())
    }
}
