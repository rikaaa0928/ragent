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
    NoopEventHandler, TokenUsage,
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

    fn image_viewer_component_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
            "extensions/image_viewer/target/wasm32-wasip2/release/ragent_image_viewer_extension.wasm",
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
            Some("你是一个高效、精准、善于深度思考的 AI 智能体助手")
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
            if matches!(event, AgentEvent::AgentFinished { .. }) {
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
                    model: ModelDraft::new("test"),
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
        assert!(success.output.to_display_string().contains("hello"));

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
        assert!(failure.output.to_display_string().contains("exit_code: 7"));

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
        assert!(timed_out
            .output
            .to_display_string()
            .contains("timed out after 1000 ms"));

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
        assert!(overridden.output.to_display_string().contains("override"));
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
                    model: ModelDraft::new("test"),
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
        assert!(result.output.to_display_string().contains("no-timeout"));
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
                    model: ModelDraft::new("test"),
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
        assert!(not_found_res
            .output
            .to_display_string()
            .contains("not found"));

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
        assert!(not_unique_res
            .output
            .to_display_string()
            .contains("not unique"));
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
        assert!(write_fail_res
            .output
            .to_display_string()
            .contains("failed to write file"));

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
        assert!(replace_fail_res
            .output
            .to_display_string()
            .contains("failed to read file"));

        let _ = std::fs::remove_file(test_file_rel);
    }

    #[tokio::test]
    async fn image_viewer_component_view_test() {
        let plugin = WasmPlugin::load_from_file("image_viewer", &image_viewer_component_path())
            .await
            .unwrap();
        let mut manager = ExtensionManager::empty();
        manager
            .add_plugin_with_config(
                plugin,
                serde_json::json!({
                    "max_file_size_bytes": 1024 * 1024
                }),
            )
            .unwrap();
        manager.initialize().await.unwrap();

        let (draft, _) = manager
            .transform_agent_draft(
                HOOK_AGENT_PREPARE,
                None,
                AgentDraft {
                    system_prompt: "test".into(),
                    model: ModelDraft::new("test"),
                    tools: vec![],
                    context: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(draft.tools.len(), 1);
        assert_eq!(draft.tools[0].definition.name, "view_image");

        // 1. 创建一个合法的 1x1 纯色 PNG 图片用于测试
        // 1x1 transparent PNG binary:
        let png_bytes: [u8; 67] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
            0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, // IHDR
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // width: 1, height: 1
            0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, // bit depth 8, RGBA
            0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, // IDAT
            0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d,
            0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, // IEND
            0x42, 0x60, 0x82,
        ];
        let test_img_path = "target/test_1x1_image.png";
        std::fs::write(test_img_path, png_bytes).unwrap();

        // 测试正常读取图片及默认附带 Base64
        let view_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "image_viewer",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-view-1".into(),
                    tool_id: "tool-view-1".into(),
                    name: "view_image".into(),
                    arguments: serde_json::json!({
                        "path": test_img_path,
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let view_res: ToolResult = serde_json::from_value(view_call).unwrap();
        assert!(view_res.success);
        assert!(view_res
            .output
            .to_display_string()
            .contains("Format: image/png"));
        assert!(view_res
            .output
            .to_display_string()
            .contains("Dimensions: 1 x 1"));
        assert!(matches!(view_res.output, ToolOutput::Parts(_)));

        // 测试 include_base64 = false 时仅输出元数据
        let view_meta_only_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "image_viewer",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-view-2".into(),
                    tool_id: "tool-view-2".into(),
                    name: "view_image".into(),
                    arguments: serde_json::json!({
                        "path": test_img_path,
                        "include_base64": false
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let view_meta_only_res: ToolResult = serde_json::from_value(view_meta_only_call).unwrap();
        assert!(view_meta_only_res.success);
        assert!(view_meta_only_res
            .output
            .to_display_string()
            .contains("Dimensions: 1 x 1"));
        assert!(matches!(view_meta_only_res.output, ToolOutput::Text(_)));

        // 测试文件不存在错误
        let not_found_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "image_viewer",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-view-3".into(),
                    tool_id: "tool-view-3".into(),
                    name: "view_image".into(),
                    arguments: serde_json::json!({
                        "path": "target/non_existent_img_xyz.png",
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let not_found_res: ToolResult = serde_json::from_value(not_found_call).unwrap();
        assert!(!not_found_res.success);
        assert!(not_found_res
            .output
            .to_display_string()
            .contains("failed to read image file"));

        // 测试目录路径传入错误
        let dir_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "image_viewer",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-view-4".into(),
                    tool_id: "tool-view-4".into(),
                    name: "view_image".into(),
                    arguments: serde_json::json!({
                        "path": "target",
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let dir_res: ToolResult = serde_json::from_value(dir_call).unwrap();
        assert!(!dir_res.success);
        assert!(dir_res
            .output
            .to_display_string()
            .contains("is a directory"));

        // 测试不支持的格式（如 SVG、BMP、GIF 等被严格拦截）
        let svg_path = "target/test_invalid_format.svg";
        std::fs::write(svg_path, "<svg width='10' height='10'></svg>").unwrap();
        let svg_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "image_viewer",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-view-5".into(),
                    tool_id: "tool-view-5".into(),
                    name: "view_image".into(),
                    arguments: serde_json::json!({
                        "path": svg_path,
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let svg_res: ToolResult = serde_json::from_value(svg_call).unwrap();
        assert!(!svg_res.success);
        assert!(svg_res
            .output
            .to_display_string()
            .contains("SVG format is not supported"));
        let _ = std::fs::remove_file(svg_path);

        // 测试 GIF 格式被明确拦截
        let gif_path = "target/test_gif_format.gif";
        let gif_bytes = [
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0xff,
            0xff, 0xff, 0x00, 0x00, 0x00, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00,
            0x3b,
        ];
        std::fs::write(gif_path, gif_bytes).unwrap();
        let gif_call = manager
            .action(
                HOOK_TOOLS_CALL,
                "image_viewer",
                None,
                serde_json::to_value(ToolCallRequest {
                    call_id: "call-view-6".into(),
                    tool_id: "tool-view-6".into(),
                    name: "view_image".into(),
                    arguments: serde_json::json!({
                        "path": gif_path,
                    }),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let gif_res: ToolResult = serde_json::from_value(gif_call).unwrap();
        assert!(!gif_res.success);
        assert!(gif_res
            .output
            .to_display_string()
            .contains("GIF format is not supported"));
        let _ = std::fs::remove_file(gif_path);

        let _ = std::fs::remove_file(test_img_path);
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
                    model: ModelDraft::new("test"),
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
        .on_event(&AgentEvent::AgentFinished { total_usage: None });
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
        JsonLinesEventHandler::new(Box::new(Writer(Arc::clone(&bytes)))).on_event(
            &AgentEvent::TurnCompleted {
                iteration: 1,
                text: "hi".into(),
                reasoning: None,
                usage: Some(TokenUsage::new(100, 50, 150, 20, 10)),
            },
        );
        let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        let parsed = serde_json::from_str::<serde_json::Value>(output.trim()).unwrap();
        assert_eq!(parsed["type"], "turn_completed");
        assert_eq!(parsed["usage"]["total_tokens"], 150);
        assert_eq!(parsed["usage"]["cached_tokens"], 20);
        assert_eq!(parsed["usage"]["reasoning_tokens"], 10);
    }

    #[test]
    fn test_reasoning_deserialization() {
        let json_str = r#"{
            "type": "reasoning",
            "id": "rs_123",
            "summary": [
                {
                    "type": "summary_text",
                    "text": "Hello thinking"
                }
            ]
        }"#;
        let item: Item = serde_json::from_str(json_str).unwrap();
        println!("Deserialized Item: {:?}", item);
    }

    #[test]
    fn token_usage_aggregation_and_formatting() {
        let mut total = TokenUsage::default();
        let u1 = TokenUsage::new(100, 50, 150, 30, 20);
        let u2 = TokenUsage::new(200, 80, 280, 50, 40);

        total += &u1;
        assert_eq!(total.input_tokens, 100);
        assert_eq!(total.output_tokens, 50);
        assert_eq!(total.total_tokens, 150);
        assert_eq!(total.cached_tokens, 30);
        assert_eq!(total.reasoning_tokens, 20);

        total += u2;
        assert_eq!(total.input_tokens, 300);
        assert_eq!(total.output_tokens, 130);
        assert_eq!(total.total_tokens, 430);
        assert_eq!(total.cached_tokens, 80);
        assert_eq!(total.reasoning_tokens, 60);

        assert_eq!(
            total.formatted_details(),
            "总计: 430 (输入: 300, 输出: 130, 缓存: 80, 思考: 60)"
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

    #[tokio::test]
    async fn config_loads_model_settings_and_reasoning_from_config_toml() {
        use openresponses_rust::{ReasoningConfig, ReasoningEffort, ReasoningSummary};

        let temp = tempfile::tempdir().unwrap();
        let config_toml = r#"
[model]
name = "gemini-2.5-pro"
temperature = 0.4
max_output_tokens = 4096

[model.reasoning]
effort = "high"
summary = "concise"

[[extensions]]
name = "shell"
path = "extensions/shell.wasm"
enabled = false
"#;
        std::fs::write(temp.path().join("config.toml"), config_toml).unwrap();

        let manager = ExtensionManager::load_from_dir(temp.path()).await.unwrap();
        assert!(manager.model_settings().is_some());

        let settings = manager.model_settings().unwrap();
        assert_eq!(settings.name.as_deref(), Some("gemini-2.5-pro"));
        assert_eq!(settings.temperature, Some(0.4));
        assert_eq!(settings.max_output_tokens, Some(4096));

        let reasoning = settings.reasoning.as_ref().unwrap();
        assert_eq!(reasoning.effort, Some(ReasoningEffort::High));
        assert_eq!(reasoning.summary, Some(ReasoningSummary::Concise));

        let mut config = AgentConfig::new("https://example.com", "fake_key", "default-model");
        config.apply_model_settings(settings);

        assert_eq!(config.model, "gemini-2.5-pro");
        assert_eq!(config.temperature, Some(0.4));
        assert_eq!(config.max_output_tokens, Some(4096));
        assert_eq!(
            config.reasoning,
            Some(ReasoningConfig {
                effort: Some(ReasoningEffort::High),
                summary: Some(ReasoningSummary::Concise),
            })
        );
    }

    #[tokio::test]
    async fn project_config_overrides_global_config_leaf_nodes() {
        use openresponses_rust::{ReasoningEffort, ReasoningSummary};

        let temp_global = tempfile::tempdir().unwrap();
        let global_config = r#"
[model]
name = "global-model"
temperature = 0.7
max_output_tokens = 2048

[model.reasoning]
effort = "medium"
summary = "auto"
"#;
        std::fs::write(temp_global.path().join("config.toml"), global_config).unwrap();

        let temp_project = tempfile::tempdir().unwrap();
        let project_dir = temp_project.path().join(".ragent");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_config = r#"
[model]
name = "project-model"
temperature = 0.2

[model.reasoning]
effort = "high"
"#;
        let project_config_file = project_dir.join("config.toml");
        std::fs::write(&project_config_file, project_config).unwrap();

        let manager =
            ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file)
                .await
                .unwrap();

        let settings = manager.model_settings().expect("settings should exist");
        // name: overridden by project
        assert_eq!(settings.name.as_deref(), Some("project-model"));
        // temperature: overridden by project
        assert_eq!(settings.temperature, Some(0.2));
        // max_output_tokens: fallback to global
        assert_eq!(settings.max_output_tokens, Some(2048));

        let reasoning = settings.reasoning.as_ref().expect("reasoning should exist");
        // effort: overridden by project
        assert_eq!(reasoning.effort, Some(ReasoningEffort::High));
        // summary: fallback to global
        assert_eq!(reasoning.summary, Some(ReasoningSummary::Auto));
    }

    #[tokio::test]
    async fn project_config_extension_restrictions_and_overrides() {
        let temp_global = tempfile::tempdir().unwrap();
        let global_config = format!(
            r#"
[[extensions]]
name = "shell"
path = {:?}
enabled = true
[extensions.config]
key1 = "val1"
nested = {{ a = 1, b = 2 }}

[[extensions]]
name = "file_editor"
path = {:?}
enabled = true
"#,
            shell_component_path().to_string_lossy(),
            file_editor_component_path().to_string_lossy()
        );
        std::fs::write(temp_global.path().join("config.toml"), global_config).unwrap();

        // 1. Valid override: modifies enabled and config on shell, disables file_editor
        let temp_project = tempfile::tempdir().unwrap();
        let project_dir = temp_project.path().join(".ragent");
        std::fs::create_dir_all(&project_dir).unwrap();
        let project_config_file = project_dir.join("config.toml");

        let project_config_valid = r#"
[[extensions]]
name = "file_editor"
enabled = false

[[extensions]]
name = "shell"
[extensions.config]
key2 = "val2"
nested = { b = 3, c = 4 }
"#;
        std::fs::write(&project_config_file, project_config_valid).unwrap();

        let manager =
            ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file)
                .await
                .unwrap();

        // Only shell is enabled, file_editor disabled
        assert_eq!(manager.plugins().len(), 1);
        assert_eq!(manager.plugins()[0].metadata().id, "shell");

        // 2. Reject non-existent extension name in project config
        let project_config_unknown_name = r#"
[[extensions]]
name = "unknown_ext"
enabled = true
"#;
        std::fs::write(&project_config_file, project_config_unknown_name).unwrap();
        let err =
            ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file)
                .await;
        let err_msg = match err {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error"),
        };
        assert!(err_msg.contains("does not exist in global config"));

        // 3. Reject fields other than name, enabled, config in project config
        let project_config_invalid_fields = r#"
[[extensions]]
name = "shell"
path = "some/other/path.wasm"
"#;
        std::fs::write(&project_config_file, project_config_invalid_fields).unwrap();
        let err =
            ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file)
                .await;
        assert!(err.is_err());

        // 4. Reject duplicate extension name in project config
        let project_config_duplicate = r#"
[[extensions]]
name = "shell"
enabled = true

[[extensions]]
name = "shell"
enabled = false
"#;
        std::fs::write(&project_config_file, project_config_duplicate).unwrap();
        let err =
            ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file)
                .await;
        let err_msg = match err {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error"),
        };
        assert!(err_msg.contains("duplicate extension name"));

        // 5. Reject duplicate extension name in global config
        let temp_global_dup = tempfile::tempdir().unwrap();
        let global_config_dup = format!(
            r#"
[[extensions]]
name = "shell"
path = {:?}

[[extensions]]
name = "shell"
path = {:?}
"#,
            shell_component_path().to_string_lossy(),
            shell_component_path().to_string_lossy()
        );
        std::fs::write(
            temp_global_dup.path().join("config.toml"),
            global_config_dup,
        )
        .unwrap();
        let err = ExtensionManager::load_with_project_config(
            temp_global_dup.path(),
            &project_config_file,
        )
        .await;
        let err_msg = match err {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error"),
        };
        assert!(err_msg.contains("duplicate extension name"));
    }
}
