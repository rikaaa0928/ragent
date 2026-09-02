use crate::domain::config::ExtensionConfigItem;
use crate::error::AgentError;
use crate::hooks::protocol::*;
use crate::hooks::runtime::WasmPlugin;
use directories::BaseDirs;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExtensionsConfigFile {
    #[serde(default)]
    pub model: Option<crate::config::ModelSettings>,
    #[serde(default)]
    pub extensions: Vec<ExtensionConfigItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectExtensionConfigItem {
    pub name: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub config: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProjectConfigRaw {
    #[serde(default)]
    pub extensions: Option<Vec<ProjectExtensionConfigItem>>,
}

#[derive(Clone)]
struct Subscriber {
    plugin: Arc<WasmPlugin>,
    subscription: HookSubscription,
}

#[derive(Debug, Clone)]
pub struct PrototypePermissionPolicy {
    pub workspace_root: PathBuf,
    pub session_tmp_dir: PathBuf,
}

fn canonicalize_or_resolve(path: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        return c;
    }
    let mut ancestors = Vec::new();
    let mut curr = path;
    while let Some(parent) = curr.parent() {
        if let Some(file_name) = curr.file_name() {
            ancestors.push(file_name);
        }
        if let Ok(canon_parent) = parent.canonicalize() {
            let mut res = canon_parent;
            for comp in ancestors.into_iter().rev() {
                res.push(comp);
            }
            return res;
        }
        curr = parent;
    }
    normalize_path(path)
}

impl PrototypePermissionPolicy {
    pub fn new(workspace_root: impl AsRef<Path>, session_tmp_dir: impl AsRef<Path>) -> Self {
        let ws = canonicalize_or_resolve(workspace_root.as_ref());
        let tmp = canonicalize_or_resolve(session_tmp_dir.as_ref());
        Self {
            workspace_root: ws,
            session_tmp_dir: tmp,
        }
    }

    pub fn is_path_allowed(&self, path: &Path) -> bool {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        };

        let check_path = canonicalize_or_resolve(&resolved);

        check_path.starts_with(&self.workspace_root)
            || check_path.starts_with(&self.session_tmp_dir)
    }

    pub fn check_file_read(&self, path: &Path) -> Result<(), String> {
        if self.is_path_allowed(path) {
            Ok(())
        } else {
            Err(format!(
                "Permission denied: reading path {:?} outside workspace {:?} and session tmp {:?}",
                path, self.workspace_root, self.session_tmp_dir
            ))
        }
    }

    pub fn check_file_write(&self, path: &Path) -> Result<(), String> {
        if self.is_path_allowed(path) {
            Ok(())
        } else {
            Err(format!(
                "Permission denied: writing path {:?} outside workspace {:?} and session tmp {:?}",
                path, self.workspace_root, self.session_tmp_dir
            ))
        }
    }

    pub fn check_command_work_dir(&self, work_dir: &Path) -> Result<(), String> {
        if self.is_path_allowed(work_dir) {
            Ok(())
        } else {
            Err(format!(
                "Permission denied: command work_dir {:?} outside workspace {:?} and session tmp {:?}",
                work_dir, self.workspace_root, self.session_tmp_dir
            ))
        }
    }

    pub fn check_http(&self) -> Result<(), String> {
        Err("Permission denied: HTTP networking is disabled in Prototype".into())
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        use std::path::Component;
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

pub struct HookManager {
    plugins: Vec<Arc<WasmPlugin>>,
    plugin_config: HashMap<String, Value>,
    config_dir: PathBuf,
    model_settings: Option<crate::config::ModelSettings>,
    permission_policy: Option<PrototypePermissionPolicy>,
    invocation_id: AtomicU64,
    tool_id: AtomicU64,
    initialized_plugins: std::sync::Mutex<Vec<Arc<WasmPlugin>>>,
}

impl HookManager {
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

    pub fn get_project_config_path() -> PathBuf {
        PathBuf::from(".ragent/config.toml")
    }

    pub fn empty() -> Self {
        Self::empty_at(Self::get_config_dir())
    }

    pub fn empty_at(config_dir: PathBuf) -> Self {
        Self {
            plugins: vec![],
            plugin_config: HashMap::new(),
            config_dir,
            model_settings: None,
            permission_policy: None,
            invocation_id: AtomicU64::new(1),
            tool_id: AtomicU64::new(1),
            initialized_plugins: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn with_permission_policy(mut self, policy: PrototypePermissionPolicy) -> Self {
        self.permission_policy = Some(policy);
        self
    }

    pub fn set_permission_policy(&mut self, policy: PrototypePermissionPolicy) {
        self.permission_policy = Some(policy);
    }

    pub fn permission_policy(&self) -> Option<&PrototypePermissionPolicy> {
        self.permission_policy.as_ref()
    }

    pub async fn load_from_default_config() -> Result<Self, AgentError> {
        Self::load_with_project_config(&Self::get_config_dir(), &Self::get_project_config_path())
            .await
    }

    pub async fn load_from_dir(config_dir: &Path) -> Result<Self, AgentError> {
        Self::load_with_project_config(config_dir, &Self::get_project_config_path()).await
    }

    pub async fn load_with_project_config(
        config_dir: &Path,
        project_config_path: &Path,
    ) -> Result<Self, AgentError> {
        let global_config = Self::load_resolved_config(config_dir, project_config_path)?;
        let mut manager = Self::empty_at(config_dir.to_path_buf());
        manager.model_settings = global_config.model;
        for item in global_config
            .extensions
            .into_iter()
            .filter(|item| item.enabled)
        {
            let path = PathBuf::from(&item.path);
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

    pub fn load_resolved_config(
        config_dir: &Path,
        project_config_path: &Path,
    ) -> Result<ExtensionsConfigFile, AgentError> {
        if !config_dir.exists() {
            let _ = fs::create_dir_all(config_dir.join("extensions"));
        }
        let global_file = config_dir.join("config.toml");
        let global_value = if global_file.exists() {
            let content = fs::read_to_string(&global_file).map_err(|e| {
                AgentError::ToolError(format!("failed to read {global_file:?}: {e}"))
            })?;
            toml::from_str::<toml::Value>(&content).map_err(|e| {
                AgentError::ToolError(format!("failed to parse {global_file:?}: {e}"))
            })?
        } else {
            toml::Value::Table(toml::map::Map::new())
        };

        let mut global_config: ExtensionsConfigFile =
            global_value.clone().try_into().map_err(|e| {
                AgentError::ToolError(format!(
                    "failed to deserialize global config {global_file:?}: {e}"
                ))
            })?;

        let mut global_names = HashSet::new();
        for item in &mut global_config.extensions {
            if !global_names.insert(item.name.clone()) {
                return Err(AgentError::ToolError(format!(
                    "duplicate extension name '{}' in global config",
                    item.name
                )));
            }

            let raw_path = &item.path;
            let expanded = if raw_path.starts_with('~') {
                if let Some(home) = dirs::home_dir() {
                    if raw_path == "~" {
                        home
                    } else if raw_path.starts_with("~/") || raw_path.starts_with("~\\") {
                        home.join(&raw_path[2..])
                    } else {
                        PathBuf::from(raw_path)
                    }
                } else {
                    PathBuf::from(raw_path)
                }
            } else {
                let p = Path::new(raw_path);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    config_dir.join(p)
                }
            };
            item.path = if let Ok(canon) = expanded.canonicalize() {
                canon.to_string_lossy().to_string()
            } else {
                expanded.to_string_lossy().to_string()
            };
        }

        if project_config_path.exists() {
            let content = fs::read_to_string(project_config_path).map_err(|e| {
                AgentError::ToolError(format!("failed to read {project_config_path:?}: {e}"))
            })?;

            let project_raw: ProjectConfigRaw = toml::from_str(&content).map_err(|e| {
                AgentError::ToolError(format!(
                    "invalid extension configuration in project config {project_config_path:?}: {e}"
                ))
            })?;

            if let Some(project_exts) = project_raw.extensions {
                let mut project_names = HashSet::new();
                for item in &project_exts {
                    if !project_names.insert(item.name.clone()) {
                        return Err(AgentError::ToolError(format!(
                            "duplicate extension name '{}' in project config {project_config_path:?}",
                            item.name
                        )));
                    }
                    if !global_names.contains(&item.name) {
                        return Err(AgentError::ToolError(format!(
                            "extension '{}' in project config {project_config_path:?} does not exist in global config",
                            item.name
                        )));
                    }
                }
            }

            let mut project_value = toml::from_str::<toml::Value>(&content).map_err(|e| {
                AgentError::ToolError(format!("failed to parse {project_config_path:?}: {e}"))
            })?;

            let project_ext_val = if let toml::Value::Table(ref mut table) = project_value {
                table.remove("extensions")
            } else {
                None
            };

            let mut merged_value = global_value;
            if let toml::Value::Table(ref mut table) = merged_value {
                table.remove("extensions");
            }
            merge_toml_value(&mut merged_value, project_value);

            let other_config: ExtensionsConfigFile = merged_value.try_into().map_err(|e| {
                AgentError::ToolError(format!("failed to deserialize merged config: {e}"))
            })?;
            global_config.model = other_config.model;

            if let Some(toml::Value::Array(project_ext_array)) = project_ext_val {
                for ext_toml in project_ext_array {
                    if let toml::Value::Table(ext_table) = ext_toml {
                        if let Some(toml::Value::String(name)) = ext_table.get("name") {
                            if let Some(global_ext) = global_config
                                .extensions
                                .iter_mut()
                                .find(|e| e.name == *name)
                            {
                                if let Some(toml::Value::Boolean(enabled)) =
                                    ext_table.get("enabled")
                                {
                                    global_ext.enabled = *enabled;
                                }
                                if let Some(config_val) = ext_table.get("config") {
                                    let json_override: Value = serde_json::to_value(config_val)
                                        .map_err(|e| {
                                            AgentError::ToolError(format!(
                                                "failed to convert extension config to json: {e}"
                                            ))
                                        })?;
                                    merge_json_value(&mut global_ext.config, json_override);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(global_config)
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

    pub fn model_settings(&self) -> Option<&crate::config::ModelSettings> {
        self.model_settings.as_ref()
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub async fn initialize(&self) -> Result<(), AgentError> {
        {
            let mut init_guard = self.initialized_plugins.lock().unwrap();
            init_guard.clear();
        }

        for plugin in &self.plugins {
            let config_val = self
                .plugin_config
                .get(&plugin.metadata().id)
                .unwrap_or(&Value::Null);

            match plugin.initialize(config_val).await {
                Ok(()) => {
                    let mut init_guard = self.initialized_plugins.lock().unwrap();
                    init_guard.push(Arc::clone(plugin));
                }
                Err(e) => {
                    let _ = self.shutdown().await;
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), AgentError> {
        let plugins_to_shutdown: Vec<Arc<WasmPlugin>> = {
            let mut guard = self.initialized_plugins.lock().unwrap();
            guard.drain(..).collect()
        };

        let mut first_error = None;
        for plugin in plugins_to_shutdown.into_iter().rev() {
            if let Err(e) = plugin.shutdown().await {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
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

        // Prototype Permission check on action payload (for tools.call)
        if hook == HOOK_TOOLS_CALL {
            if let Some(ref policy) = self.permission_policy {
                if let Ok(call_req) = serde_json::from_value::<ToolCallRequest>(payload.clone()) {
                    // Check file_editor and image_viewer path parameters
                    if let Some(path_str) = call_req.arguments.get("path").and_then(Value::as_str) {
                        let path = Path::new(path_str);
                        if call_req.name == "write_file" || call_req.name == "replace_in_file" {
                            if let Err(e) = policy.check_file_write(path) {
                                return Ok(serde_json::to_value(ToolResult::err(e))?);
                            }
                        } else if call_req.name == "view_image" {
                            if let Err(e) = policy.check_file_read(path) {
                                return Ok(serde_json::to_value(ToolResult::err(e))?);
                            }
                        }
                    }
                    // Check shell command work_dir
                    if let Some(work_dir_str) =
                        call_req.arguments.get("work_dir").and_then(Value::as_str)
                    {
                        let work_dir = Path::new(work_dir_str);
                        if let Err(e) = policy.check_command_work_dir(work_dir) {
                            return Ok(serde_json::to_value(ToolResult::err(e))?);
                        }
                    }
                }
            }
        }

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

fn merge_toml_value(base: &mut toml::Value, override_val: toml::Value) {
    match (base, override_val) {
        (toml::Value::Table(base_table), toml::Value::Table(override_table)) => {
            for (k, v) in override_table {
                if let Some(base_entry) = base_table.get_mut(&k) {
                    merge_toml_value(base_entry, v);
                } else {
                    base_table.insert(k, v);
                }
            }
        }
        (base_slot, override_val) => {
            *base_slot = override_val;
        }
    }
}

fn merge_json_value(base: &mut Value, override_val: Value) {
    match (base, override_val) {
        (Value::Object(base_map), Value::Object(override_map)) => {
            for (k, v) in override_map {
                if let Some(base_entry) = base_map.get_mut(&k) {
                    merge_json_value(base_entry, v);
                } else {
                    base_map.insert(k, v);
                }
            }
        }
        (base_slot, override_val) => {
            *base_slot = override_val;
        }
    }
}
