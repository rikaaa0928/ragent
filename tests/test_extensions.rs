mod common;

use common::{file_editor_component_path, image_viewer_component_path, shell_component_path};
use ragent::{
    AgentDraft, ExtensionManager, ModelDraft, ToolCallRequest, ToolOutput, ToolResult, WasmPlugin,
    HOOK_AGENT_PREPARE, HOOK_TOOLS_CALL,
};

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
