pub mod agent;
pub mod builder;
pub mod cli;
pub mod config;
pub mod context;
pub mod error;
pub mod event;
pub mod sender;
pub mod session;
pub mod wasm;

pub use agent::Agent;
pub use builder::AgentBuilder;
pub use config::AgentConfig;
pub use context::AgentContext;
pub use error::AgentError;
pub use event::{
    AgentEvent, ConsoleEventHandler, EventHandler, FnEventHandler, JsonLinesEventHandler,
    NoopEventHandler,
};
pub use sender::AgentSender;
pub use session::{SessionData, SessionMeta, SessionStore};
pub use wasm::{
    ExtensionConfigItem, ExtensionManager, ExtensionMetadata, ExtensionsConfig, HookFailurePolicy,
    HookKind, HookRequest, HookSubscription, ToolCallRequest, ToolDefinition, ToolResult,
    ToolsListResult, WasmPlugin,
};

pub use openresponses_rust::{Item, Tool};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn shell_component_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("extensions/shell/target/component/ragent_shell_extension.wasm")
    }

    #[tokio::test]
    async fn builder_and_context_do_not_duplicate_system_prompt() {
        let config = AgentConfig::new("https://example.com", "fake_key", "test-model")
            .with_max_iterations(10)
            .with_temperature(0.5);
        let (agent, _) = AgentBuilder::new(config)
            .with_extension_manager(ExtensionManager::empty())
            .with_system_prompt("system")
            .build()
            .await
            .unwrap();

        assert_eq!(agent.context().system_prompt(), Some("system"));
        assert!(agent.context().items().is_empty());
    }

    #[test]
    fn context_keeps_complete_history() {
        let mut context =
            AgentContext::from_existing(vec![Item::system_message("old system")], None);
        for index in 1..=6 {
            context.add_user_message(format!("message {index}"));
        }
        assert_eq!(context.items().len(), 6);
        assert_eq!(context.system_prompt(), Some("old system"));
    }

    #[tokio::test]
    async fn shell_component_reports_command_failure() {
        let plugin = WasmPlugin::load_from_file("shell", &shell_component_path())
            .await
            .unwrap();
        let mut manager = ExtensionManager::empty();
        manager.add_plugin(plugin).unwrap();
        manager.initialize().await.unwrap();

        let (tools, owners) = manager.resolve_tools(serde_json::json!({})).await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "shell");

        let success = manager
            .execute_tool(
                &owners,
                "shell",
                serde_json::json!({"command": "printf hello"}),
            )
            .await
            .unwrap();
        assert!(success.success);
        assert!(success.output.contains("hello"));

        let failure = manager
            .execute_tool(&owners, "shell", serde_json::json!({"command": "exit 7"}))
            .await
            .unwrap();
        assert!(!failure.success);
        assert!(failure.error.is_some());
        assert!(failure.output.contains("exit_code: 7"));
    }

    #[tokio::test]
    async fn extension_manager_loads_component_from_bootstrap_config() {
        let temp = tempfile::tempdir().unwrap();
        let config = format!(
            "[[extensions]]\nname = \"shell\"\npath = {:?}\nenabled = true\n",
            shell_component_path().to_string_lossy()
        );
        std::fs::write(temp.path().join("config.toml"), config).unwrap();

        let manager = ExtensionManager::load_from_dir(temp.path()).await.unwrap();
        manager.initialize().await.unwrap();
        let (tools, _) = manager.resolve_tools(serde_json::json!({})).await.unwrap();
        assert_eq!(tools[0].name, "shell");
    }

    #[test]
    fn event_handler_closure_and_jsonl_work() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};

        let counter = Arc::new(AtomicUsize::new(0));
        let cloned = Arc::clone(&counter);
        FnEventHandler(move |_| {
            cloned.fetch_add(1, Ordering::SeqCst);
        })
        .on_event(&AgentEvent::AgentFinished);
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        let bytes = Arc::new(Mutex::new(Vec::new()));
        struct Writer(Arc<Mutex<Vec<u8>>>);
        impl Write for Writer {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        JsonLinesEventHandler::new(Box::new(Writer(Arc::clone(&bytes))))
            .on_event(&AgentEvent::TextDelta { delta: "hi".into() });
        let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(output.trim()).unwrap()["type"],
            "text_delta"
        );
    }

    #[test]
    fn session_store_crud_and_rejects_path_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path().join("sessions"));
        let mut session = SessionData::new("sess_test", "test-model", Some("system".into()));
        session.update_from_context(vec![Item::user_message("What is Rust?")]);
        store.save(&session).unwrap();

        let loaded = store.load("sess_test").unwrap().unwrap();
        assert_eq!(loaded.meta.item_count, 1);
        assert_eq!(store.list().unwrap().len(), 1);
        assert!(store.delete("sess_test").unwrap());

        assert!(matches!(
            store.load("../escape"),
            Err(AgentError::InvalidSessionId(_))
        ));
        let escaped = SessionData::new("../escape", "model", None);
        assert!(matches!(
            store.save(&escaped),
            Err(AgentError::InvalidSessionId(_))
        ));
        assert!(!temp.path().join("escape.json").exists());
    }
}
