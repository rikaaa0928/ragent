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
pub use wasm::types::*;
pub use wasm::{ExtensionConfigItem, ExtensionManager, ExtensionsConfig, WasmPlugin};

pub use openresponses_rust::{Item, Tool};

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn shell_component_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("extensions/shell/target/wasm32-wasip2/release/ragent_shell_extension.wasm")
    }

    fn file_editor_component_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "extensions/file_editor/target/wasm32-wasip2/release/ragent_file_editor_extension.wasm",
        )
    }

    #[tokio::test]
    async fn builder_and_context_do_not_duplicate_system_prompt() {
        let config = AgentConfig::new("https://example.com", "fake_key", "test-model")
            .with_max_iterations(10)
            .with_temperature(0.5);
        let (agent, _) = AgentBuilder::new(config)
            .with_extension_manager(ExtensionManager::empty())
            .build()
            .await
            .unwrap();

        assert_eq!(
            agent.context().system_prompt(),
            Some("你是一个高效、精准的 AI 智能体助手")
        );
        assert!(agent.context().items().is_empty());
    }

    #[tokio::test]
    async fn sender_cancels_agent_before_model_io() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let config = AgentConfig::new("https://example.invalid", "fake_key", "test-model");
        let (mut agent, sender) = AgentBuilder::new(config)
            .with_extension_manager(ExtensionManager::empty())
            .build()
            .await
            .unwrap();
        let finished = Arc::new(AtomicUsize::new(0));
        let finished_for_handler = Arc::clone(&finished);
        agent.set_event_handler(Arc::new(FnEventHandler(move |event| {
            if matches!(event, AgentEvent::AgentFinished) {
                finished_for_handler.fetch_add(1, Ordering::SeqCst);
            }
        })));
        agent.add_user_message("this must not reach the model");

        sender.cancel();

        assert!(sender.is_cancelled());
        assert!(sender.cancellation_token().is_cancelled());
        assert_eq!(agent.run().await.unwrap(), "");
        assert_eq!(finished.load(Ordering::SeqCst), 1);
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
        manager
            .add_plugin_with_config(plugin, serde_json::json!({"default_timeout_seconds": 1}))
            .unwrap();
        manager.initialize().await.unwrap();

        let (draft, _) = manager
            .transform_agent_draft(
                HOOK_AGENT_PREPARE,
                None,
                AgentDraft {
                    system_prompt: "test".into(),
                    model: ModelDraft {
                        name: "test".into(),
                        temperature: None,
                        max_output_tokens: None,
                    },
                    tools: vec![],
                    context: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(draft.tools.len(), 1);
        assert_eq!(draft.tools[0].definition.name, "shell");
        let tool = &draft.tools[0];
        assert_eq!(
            tool.definition.parameters["properties"]["timeout_seconds"]["default"],
            1
        );

        let success = manager
            .action(
                HOOK_TOOLS_CALL,
                tool.owner.as_deref().unwrap(),
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-1".into(),
                    tool_id: tool.id.clone().unwrap(),
                    name: "shell".into(),
                    arguments: serde_json::json!({"command": "printf hello"}),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let success: ToolResult = serde_json::from_value(success).unwrap();
        assert!(success.success);
        assert!(success.output.contains("hello"));

        let failure = manager
            .action(
                HOOK_TOOLS_CALL,
                tool.owner.as_deref().unwrap(),
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-2".into(),
                    tool_id: tool.id.clone().unwrap(),
                    name: "shell".into(),
                    arguments: serde_json::json!({"command": "exit 7"}),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let failure: ToolResult = serde_json::from_value(failure).unwrap();
        assert!(!failure.success);
        assert!(failure.error.is_some());
        assert!(failure.output.contains("exit_code: 7"));

        let timed_out = manager
            .action(
                HOOK_TOOLS_CALL,
                tool.owner.as_deref().unwrap(),
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-3".into(),
                    tool_id: tool.id.clone().unwrap(),
                    name: "shell".into(),
                    arguments: serde_json::json!({"command": "sleep 2"}),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let timed_out: ToolResult = serde_json::from_value(timed_out).unwrap();
        assert!(!timed_out.success);
        assert!(timed_out.output.contains("timed out after 1000 ms"));

        let overridden = manager
            .action(
                HOOK_TOOLS_CALL,
                tool.owner.as_deref().unwrap(),
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-4".into(),
                    tool_id: tool.id.clone().unwrap(),
                    name: "shell".into(),
                    arguments: serde_json::json!({
                        "command": "sleep 1; printf override",
                        "timeout_seconds": 2
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let overridden: ToolResult = serde_json::from_value(overridden).unwrap();
        assert!(overridden.success);
        assert!(overridden.output.contains("override"));
    }

    #[tokio::test]
    async fn shell_component_accepts_zero_to_disable_timeout() {
        let plugin = WasmPlugin::load_from_file("shell", &shell_component_path())
            .await
            .unwrap();
        let mut manager = ExtensionManager::empty();
        manager
            .add_plugin_with_config(plugin, serde_json::json!({"default_timeout_seconds": 0}))
            .unwrap();

        manager.initialize().await.unwrap();
        let (draft, _) = manager
            .transform_agent_draft(
                HOOK_AGENT_PREPARE,
                None,
                AgentDraft {
                    system_prompt: "test".into(),
                    model: ModelDraft {
                        name: "test".into(),
                        temperature: None,
                        max_output_tokens: None,
                    },
                    tools: vec![],
                    context: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            draft.tools[0].definition.parameters["properties"]["timeout_seconds"]["default"],
            0
        );
        let tool = &draft.tools[0];
        let result = manager
            .action(
                HOOK_TOOLS_CALL,
                tool.owner.as_deref().unwrap(),
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-no-timeout".into(),
                    tool_id: tool.id.clone().unwrap(),
                    name: "shell".into(),
                    arguments: serde_json::json!({"command": "sleep 1; printf no-timeout"}),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let result: ToolResult = serde_json::from_value(result).unwrap();
        assert!(result.success);
        assert!(result.output.contains("no-timeout"));
    }

    #[tokio::test]
    async fn file_editor_component_write_and_replace_test() {
        let plugin = WasmPlugin::load_from_file("file_editor", &file_editor_component_path())
            .await
            .unwrap();
        let mut manager = ExtensionManager::empty();
        manager.add_plugin(plugin).unwrap();
        manager.initialize().await.unwrap();

        let (draft, _) = manager
            .transform_agent_draft(
                HOOK_AGENT_PREPARE,
                None,
                AgentDraft {
                    system_prompt: "test".into(),
                    model: ModelDraft {
                        name: "test".into(),
                        temperature: None,
                        max_output_tokens: None,
                    },
                    tools: vec![],
                    context: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(draft.tools.len(), 2);
        assert!(draft
            .tools
            .iter()
            .any(|t| t.definition.name == "write_file"));
        assert!(draft
            .tools
            .iter()
            .any(|t| t.definition.name == "replace_in_file"));

        let test_file_rel = "target/test_file_editor_temp.txt";
        let _ = std::fs::remove_file(test_file_rel);

        // 1. 测试 write_file
        let write_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "file_editor",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-wf".into(),
                    tool_id: "tool-wf".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({
                        "path": test_file_rel,
                        "content": "line 1\nline 2\nline 3\nline 2\n"
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let write_res: ToolResult = serde_json::from_value(write_call).unwrap();
        assert!(write_res.success);
        assert_eq!(
            std::fs::read_to_string(test_file_rel).unwrap(),
            "line 1\nline 2\nline 3\nline 2\n"
        );

        // 2. 测试 replace_in_file 唯一定位替换
        let replace_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "file_editor",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-rf-1".into(),
                    tool_id: "tool-rf-1".into(),
                    name: "replace_in_file".into(),
                    arguments: serde_json::json!({
                        "path": test_file_rel,
                        "replacements": [
                            {
                                "old_str": "line 1\nline 2",
                                "new_str": "first line\nsecond line"
                            }
                        ]
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let replace_res: ToolResult = serde_json::from_value(replace_call).unwrap();
        assert!(replace_res.success);
        assert_eq!(
            std::fs::read_to_string(test_file_rel).unwrap(),
            "first line\nsecond line\nline 3\nline 2\n"
        );
        // 3. 测试 replace_in_file 未找到 old_str 报错
        let not_found_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "file_editor",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-rf-2".into(),
                    tool_id: "tool-rf-2".into(),
                    name: "replace_in_file".into(),
                    arguments: serde_json::json!({
                        "path": test_file_rel,
                        "replacements": [
                            {
                                "old_str": "non_existent_string",
                                "new_str": "new_string"
                            }
                        ]
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let not_found_res: ToolResult = serde_json::from_value(not_found_call).unwrap();
        assert!(!not_found_res.success);
        assert!(not_found_res.output.contains("not found"));

        // 4. 测试 replace_in_file 命中多处 (不唯一) 报错拦截且未修改文件
        let content_before = std::fs::read_to_string(test_file_rel).unwrap();
        let not_unique_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "file_editor",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-rf-3".into(),
                    tool_id: "tool-rf-3".into(),
                    name: "replace_in_file".into(),
                    arguments: serde_json::json!({
                        "path": test_file_rel,
                        "replacements": [
                            {
                                "old_str": "line", // 在当前文件中出现了多次 (second line, line 3, line 2)
                                "new_str": "LINE"
                            }
                        ]
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let not_unique_res: ToolResult = serde_json::from_value(not_unique_call).unwrap();
        assert!(!not_unique_res.success);
        assert!(not_unique_res.output.contains("not unique"));
        assert_eq!(
            std::fs::read_to_string(test_file_rel).unwrap(),
            content_before
        );

        // 5. 测试写入失败路径（如写入目标路径为已存在的目录）
        let write_fail_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "file_editor",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-wf-fail".into(),
                    tool_id: "tool-wf-fail".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({
                        "path": "target", // target 是一个已有目录
                        "content": "some content"
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let write_fail_res: ToolResult = serde_json::from_value(write_fail_call).unwrap();
        assert!(!write_fail_res.success);
        assert!(write_fail_res.output.contains("failed to write file"));

        // 6. 测试增量编辑失败路径（如目标文件不存在）
        let replace_fail_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "file_editor",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-rf-fail".into(),
                    tool_id: "tool-rf-fail".into(),
                    name: "replace_in_file".into(),
                    arguments: serde_json::json!({
                        "path": "target/non_existent_file_for_replace.txt",
                        "replacements": [
                            {
                                "old_str": "abc",
                                "new_str": "def"
                            }
                        ]
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let replace_fail_res: ToolResult = serde_json::from_value(replace_fail_call).unwrap();
        assert!(!replace_fail_res.success);
        assert!(replace_fail_res.output.contains("failed to read file"));

        let _ = std::fs::remove_file(test_file_rel);
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
        let (draft, _) = manager
            .transform_agent_draft(
                HOOK_AGENT_PREPARE,
                None,
                AgentDraft {
                    system_prompt: "test".into(),
                    model: ModelDraft {
                        name: "test".into(),
                        temperature: None,
                        max_output_tokens: None,
                    },
                    tools: vec![],
                    context: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(draft.tools[0].definition.name, "shell");
        assert_eq!(
            draft.tools[0].definition.parameters["properties"]["timeout_seconds"]["default"],
            1800
        );
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
