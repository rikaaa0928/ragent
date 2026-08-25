use crate::error::AgentError;
use crate::wasm::runtime::WasmPlugin;
use crate::wasm::types::*;
use directories::BaseDirs;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    invocation_id: AtomicU64,
    tool_id: AtomicU64,
}

impl ExtensionManager {
    pub fn get_config_dir() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.trim().is_empty() {
                return PathBuf::from(xdg).join("ragent");
            }
        }
        if let Some(home) = dirs::home_dir() {
            home.join(".config/ragent")
        } else if let Some(base) = BaseDirs::new() {
            base.config_dir().join("ragent")
        } else {
            PathBuf::from(".config/ragent")
        }
    }

    pub async fn load_from_default_config() -> Result<Self, AgentError> {
        Self::load_from_dir(&Self::get_config_dir()).await
    }

    pub async fn load_from_dir(config_dir: &Path) -> Result<Self, AgentError> {
        if !config_dir.exists() {
            fs::create_dir_all(config_dir.join("extensions")).map_err(|e| {
                AgentError::ToolError(format!("failed to create extension config dir: {e}"))
            })?;
        }
        let file = config_dir.join("config.toml");
        let config = if file.exists() {
            toml::from_str::<ExtensionsConfig>(
                &fs::read_to_string(&file)
                    .map_err(|e| AgentError::ToolError(format!("failed to read {file:?}: {e}")))?,
            )
            .map_err(|e| AgentError::ToolError(format!("failed to parse {file:?}: {e}")))?
        } else {
            ExtensionsConfig::default()
        };

        let mut manager = Self::empty_at(config_dir.to_path_buf());
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

    fn empty_at(config_dir: PathBuf) -> Self {
        Self {
            plugins: vec![],
            plugin_config: HashMap::new(),
            config_dir,
            invocation_id: AtomicU64::new(1),
            tool_id: AtomicU64::new(1),
        }
    }

    pub fn empty() -> Self {
        Self::empty_at(Self::get_config_dir())
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
        if self.plugins.iter().any(|p| p.metadata().id == id) {
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
            plugin
                .initialize(
                    self.plugin_config
                        .get(&plugin.metadata().id)
                        .unwrap_or(&Value::Null),
                )
                .await?;
        }
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<(), AgentError> {
        for result in join_all(self.plugins.iter().map(|p| p.shutdown())).await {
            result?;
        }
        Ok(())
    }

    fn request(&self, hook: &str, iteration: Option<usize>, payload: Value) -> HookRequest {
        HookRequest {
            hook: hook.into(),
            protocol_version: 1,
            invocation_id: self.invocation_id.fetch_add(1, Ordering::Relaxed),
            iteration,
            payload,
        }
    }

    pub async fn transform(
        &self,
        hook: &str,
        iteration: Option<usize>,
        payload: Value,
    ) -> Result<TransformResult, AgentError> {
        self.transform_validated(hook, iteration, payload, |_| Ok(()))
            .await
    }

    pub async fn transform_validated<F>(
        &self,
        hook: &str,
        iteration: Option<usize>,
        mut payload: Value,
        validate: F,
    ) -> Result<TransformResult, AgentError>
    where
        F: Fn(&Value) -> Result<(), String>,
    {
        validate(&payload).map_err(|reason| {
            AgentError::ToolError(format!(
                "core produced invalid payload for '{hook}': {reason}"
            ))
        })?;
        for sub in self.subscribers(hook, HookKind::Transform) {
            let before = payload.clone();
            let response = sub
                .plugin
                .invoke(&self.request(hook, iteration, before.clone()))
                .await
                .and_then(parse_hook_result);
            let result = match response {
                Ok(result) => result,
                Err(_) if sub.subscription.failure == HookFailurePolicy::Ignore => continue,
                Err(e) => return Err(extension_error(&sub, hook, e)),
            };
            match result.action {
                HookAction::Continue => {
                    let candidate = result
                        .payload
                        .ok_or_else(|| extension_error(&sub, hook, "continue requires payload"))?;
                    if let Err(reason) = validate(&candidate) {
                        if sub.subscription.failure == HookFailurePolicy::Ignore {
                            continue;
                        }
                        return Err(extension_error(&sub, hook, reason));
                    }
                    payload = candidate;
                }
                HookAction::Unchanged => {}
                HookAction::Reject => {
                    return Err(AgentError::HookRejected {
                        hook: hook.into(),
                        reason: result
                            .reason
                            .unwrap_or_else(|| sub.plugin.metadata().id.clone()),
                    })
                }
                HookAction::Skip => {
                    return Ok(TransformResult {
                        payload,
                        control: FlowControl::Skip,
                    })
                }
                HookAction::Stop => {
                    return Ok(TransformResult {
                        payload,
                        control: FlowControl::Stop,
                    })
                }
            }
        }
        Ok(TransformResult {
            payload,
            control: FlowControl::Continue,
        })
    }

    pub async fn transform_agent_draft(
        &self,
        hook: &str,
        iteration: Option<usize>,
        draft: AgentDraft,
    ) -> Result<(AgentDraft, FlowControl), AgentError> {
        let mut payload = serde_json::to_value(draft)?;
        for sub in self.subscribers(hook, HookKind::Transform) {
            let before_value = payload.clone();
            let before: AgentDraft = serde_json::from_value(before_value.clone())?;
            let response = sub
                .plugin
                .invoke(&self.request(hook, iteration, before_value))
                .await
                .and_then(parse_hook_result);
            let result = match response {
                Ok(v) => v,
                Err(_) if sub.subscription.failure == HookFailurePolicy::Ignore => continue,
                Err(e) => return Err(extension_error(&sub, hook, e)),
            };
            let control = match result.action {
                HookAction::Continue => {
                    let candidate = result
                        .payload
                        .ok_or_else(|| extension_error(&sub, hook, "continue requires payload"))?;
                    match serde_json::from_value::<AgentDraft>(candidate).and_then(|mut draft| {
                        self.validate_agent_draft(&before, &mut draft, &sub.plugin.metadata().id)
                            .map(|_| serde_json::to_value(draft).expect("serializable draft"))
                            .map_err(serde_json::Error::io)
                    }) {
                        Ok(v) => payload = v,
                        Err(_) if sub.subscription.failure == HookFailurePolicy::Ignore => continue,
                        Err(e) => return Err(extension_error(&sub, hook, e)),
                    }
                    FlowControl::Continue
                }
                HookAction::Unchanged => FlowControl::Continue,
                HookAction::Reject => {
                    return Err(AgentError::HookRejected {
                        hook: hook.into(),
                        reason: result
                            .reason
                            .unwrap_or_else(|| sub.plugin.metadata().id.clone()),
                    })
                }
                HookAction::Skip => FlowControl::Skip,
                HookAction::Stop => FlowControl::Stop,
            };
            if control != FlowControl::Continue {
                return Ok((serde_json::from_value(payload)?, control));
            }
        }
        let draft: AgentDraft = serde_json::from_value(payload)?;
        Ok((draft, FlowControl::Continue))
    }

    fn validate_agent_draft(
        &self,
        before: &AgentDraft,
        after: &mut AgentDraft,
        current_extension: &str,
    ) -> Result<(), std::io::Error> {
        after.context = before.context.clone();
        if after.system_prompt.trim().is_empty() {
            return invalid("system_prompt must not be empty");
        }
        if after.model.name.trim().is_empty() {
            return invalid("model name must not be empty");
        }
        if after
            .model
            .temperature
            .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
        {
            return invalid("temperature must be between 0 and 2");
        }
        if after.model.max_output_tokens.is_some_and(|v| v <= 0) {
            return invalid("max_output_tokens must be positive");
        }
        let existing: HashMap<_, _> = before
            .tools
            .iter()
            .filter_map(|t| t.id.as_ref().map(|id| (id.clone(), t.owner.clone())))
            .collect();
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for tool in &mut after.tools {
            if tool.definition.name.trim().is_empty() {
                return invalid("tool name must not be empty");
            }
            if tool
                .definition
                .parameters
                .get("type")
                .and_then(Value::as_str)
                != Some("object")
            {
                return invalid("tool parameters must be a JSON schema object");
            }
            match &tool.id {
                Some(id) => {
                    let owner = existing.get(id).ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("extension supplied unknown tool id '{id}'"),
                        )
                    })?;
                    tool.owner = owner.clone();
                }
                None => {
                    if !self.plugins.iter().any(|plugin| {
                        plugin.metadata().id == current_extension
                            && plugin.metadata().subscriptions.iter().any(|subscription| {
                                subscription.hook == HOOK_TOOLS_CALL
                                    && subscription.kind == HookKind::Action
                            })
                    }) {
                        return invalid(format!(
                            "extension '{current_extension}' added a tool without owning tools.call"
                        ));
                    }
                    let id = format!("tool-{}", self.tool_id.fetch_add(1, Ordering::Relaxed));
                    tool.id = Some(id);
                    tool.owner = Some(current_extension.to_string());
                }
            }
            if !ids.insert(tool.id.clone().unwrap()) {
                return invalid("duplicate tool id");
            }
            if !names.insert(tool.definition.name.clone()) {
                return invalid("duplicate tool name");
            }
        }
        Ok(())
    }

    pub async fn action(
        &self,
        hook: &str,
        owner: &str,
        iteration: Option<usize>,
        payload: Value,
    ) -> Result<Value, AgentError> {
        let sub = self
            .subscribers(hook, HookKind::Action)
            .into_iter()
            .find(|s| s.plugin.metadata().id == owner)
            .ok_or_else(|| {
                AgentError::ToolError(format!("extension '{owner}' does not own action '{hook}'"))
            })?;
        let result = sub
            .plugin
            .invoke(&self.request(hook, iteration, payload))
            .await
            .and_then(parse_hook_result)
            .map_err(|e| extension_error(&sub, hook, e))?;
        match result.action {
            HookAction::Continue => result
                .payload
                .ok_or_else(|| extension_error(&sub, hook, "continue requires payload")),
            HookAction::Unchanged => Err(extension_error(
                &sub,
                hook,
                "action cannot return unchanged",
            )),
            HookAction::Reject => Err(AgentError::HookRejected {
                hook: hook.into(),
                reason: result.reason.unwrap_or_else(|| owner.into()),
            }),
            HookAction::Skip => Err(extension_error(&sub, hook, "action cannot return skip")),
            HookAction::Stop => Err(extension_error(&sub, hook, "action cannot return stop")),
        }
    }

    pub async fn observe(
        &self,
        hook: &str,
        iteration: Option<usize>,
        payload: Value,
    ) -> Result<(), AgentError> {
        let futures = self
            .subscribers(hook, HookKind::Observer)
            .into_iter()
            .map(|sub| {
                let request = self.request(hook, iteration, payload.clone());
                async move {
                    match sub.plugin.invoke(&request).await {
                        Ok(_) => Ok(()),
                        Err(_) if sub.subscription.failure == HookFailurePolicy::Ignore => Ok(()),
                        Err(e) => Err(extension_error(&sub, hook, e)),
                    }
                }
            });
        for result in join_all(futures).await {
            result?;
        }
        Ok(())
    }

    fn subscribers(&self, hook: &str, kind: HookKind) -> Vec<Subscriber> {
        let mut out = self
            .plugins
            .iter()
            .flat_map(|plugin| {
                plugin
                    .metadata()
                    .subscriptions
                    .iter()
                    .filter(move |s| s.hook == hook && s.kind == kind)
                    .map(move |s| Subscriber {
                        plugin: Arc::clone(plugin),
                        subscription: s.clone(),
                    })
            })
            .collect::<Vec<_>>();
        out.sort_by_key(|s| s.subscription.priority);
        out
    }

    pub fn validate_subscriptions(&self) -> Result<(), AgentError> {
        let mut seen = HashSet::new();
        for plugin in &self.plugins {
            if plugin.metadata().id.trim().is_empty() {
                return Err(AgentError::ToolError(
                    "extension id must not be empty".into(),
                ));
            }
            for sub in &plugin.metadata().subscriptions {
                if !seen.insert((plugin.metadata().id.clone(), sub.hook.clone(), sub.kind)) {
                    return Err(AgentError::ToolError(format!(
                        "extension '{}' declares hook '{}' more than once",
                        plugin.metadata().id,
                        sub.hook
                    )));
                }
            }
        }
        Ok(())
    }
}

fn parse_hook_result(value: Value) -> Result<HookResult, AgentError> {
    Ok(serde_json::from_value(value)?)
}
fn extension_error(sub: &Subscriber, hook: &str, error: impl std::fmt::Display) -> AgentError {
    AgentError::ToolError(format!(
        "extension '{}' failed at '{hook}': {error}",
        sub.plugin.metadata().id
    ))
}
fn invalid<T>(message: impl Into<String>) -> Result<T, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    ))
}
