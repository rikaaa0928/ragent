use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::error::AgentError;
use crate::event::EventHandler;
use crate::sender::AgentSender;
use crate::session::SessionData;
use crate::wasm::{ExtensionManager, WasmPlugin};
use std::path::Path;
use std::sync::Arc;

/// Agent 极简构建器
pub struct AgentBuilder {
    config: AgentConfig,
    initial_user_messages: Vec<String>,
    event_handler: Option<Arc<dyn EventHandler>>,
    extension_manager: Option<ExtensionManager>,
}

impl AgentBuilder {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config,
            initial_user_messages: Vec::new(),
            event_handler: None,
            extension_manager: None,
        }
    }

    /// 覆盖配置中的模型名称
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.config = self.config.with_model(model);
        self
    }

    pub fn with_user_message(mut self, message: impl Into<String>) -> Self {
        self.initial_user_messages.push(message.into());
        self
    }

    pub fn with_event_handler(mut self, handler: Arc<dyn EventHandler>) -> Self {
        self.event_handler = Some(handler);
        self
    }

    pub fn with_extension_manager(mut self, manager: ExtensionManager) -> Self {
        self.extension_manager = Some(manager);
        self
    }

    pub async fn with_wasm_plugin_file(
        mut self,
        name: &str,
        path: &Path,
    ) -> Result<Self, AgentError> {
        let plugin = WasmPlugin::load_from_file(name, path).await?;
        let mut manager = self
            .extension_manager
            .unwrap_or_else(ExtensionManager::empty);
        manager.add_plugin(plugin)?;
        self.extension_manager = Some(manager);
        Ok(self)
    }

    /// 从历史 SessionData 构建 Agent
    pub async fn from_session(
        session: SessionData,
        config: AgentConfig,
    ) -> Result<(Agent, AgentSender), AgentError> {
        Self::from_session_with_manager(session, config, None).await
    }

    /// 从历史 SessionData 与指定的 ExtensionManager 构建 Agent
    pub async fn from_session_with_manager(
        session: SessionData,
        config: AgentConfig,
        extension_manager: Option<ExtensionManager>,
    ) -> Result<(Agent, AgentSender), AgentError> {
        let mut builder = Self::new(config);
        if let Some(mgr) = extension_manager {
            builder = builder.with_extension_manager(mgr);
        }
        let (mut agent, sender) = builder.build().await?;
        let prompt = agent.context().system_prompt().map(str::to_owned);
        *agent.context_mut() = crate::context::AgentContext::from_existing(session.items, prompt);
        Ok((agent, sender))
    }

    /// 最终异步组装并创建 Agent 和 Sender
    pub async fn build(self) -> Result<(Agent, AgentSender), AgentError> {
        let manager = match self.extension_manager {
            Some(manager) => manager,
            None => ExtensionManager::load_from_default_config().await?,
        };

        let (mut agent, sender) = Agent::new_with_manager(self.config, manager).await?;

        for user_msg in self.initial_user_messages {
            agent.add_user_message(user_msg);
        }

        if let Some(handler) = self.event_handler {
            agent.set_event_handler(handler);
        }

        Ok((agent, sender))
    }
}
