mod common;

use common::{file_editor_component_path, shell_component_path};
use openresponses_rust::{Item, Usage};
use ragent::control::service::ControlService;
use ragent::domain::config::{ConfigRevision, ExtensionConfigItem};
use ragent::domain::ids::SessionId;
use ragent::domain::session::SessionSpec;
use ragent::domain::workspace::WorkspaceSpec;
use ragent::hooks::manager::{HookManager, PrototypePermissionPolicy};
use ragent::hooks::protocol::*;
use ragent::hooks::runtime::WasmPlugin;
use ragent::store::sqlite::SqliteControlStore;
use ragent::AgentConfig;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

fn make_json_response(id: &str, output: Vec<Item>, usage: Option<Usage>) -> serde_json::Value {
    let output_val = serde_json::to_value(output).unwrap();
    let usage_val = usage
        .map(|u| serde_json::to_value(u).unwrap())
        .unwrap_or(serde_json::Value::Null);

    serde_json::json!({
        "id": id,
        "object": "response",
        "created_at": 1700000000,
        "status": "completed",
        "model": "test-model",
        "output": output_val,
        "tools": [],
        "tool_choice": "auto",
        "truncation": "disabled",
        "parallel_tool_calls": true,
        "text": { "format": { "type": "text" } },
        "top_p": 1.0,
        "presence_penalty": 0.0,
        "frequency_penalty": 0.0,
        "top_logprobs": 0,
        "temperature": 1.0,
        "store": false,
        "background": false,
        "service_tier": "default",
        "metadata": {},
        "usage": usage_val
    })
}

async fn start_mock_server(
    responses: Vec<serde_json::Value>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://127.0.0.1:{}", addr.port());

    let response_index = Arc::new(AtomicUsize::new(0));
    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };

            let responses = responses.clone();
            let idx_atomic = Arc::clone(&response_index);

            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let _ = socket.read(&mut buf).await;

                let idx = idx_atomic.fetch_add(1, Ordering::SeqCst);
                let resp_val = if idx < responses.len() {
                    responses[idx].clone()
                } else {
                    responses
                        .last()
                        .cloned()
                        .unwrap_or_else(|| make_json_response("resp_fallback", vec![], None))
                };

                let body = serde_json::to_string(&resp_val).unwrap();
                let response_str = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response_str.as_bytes()).await;
                let _ = socket.flush().await;
                let _ = socket.shutdown().await;
            });
        }
    });

    (url, handle)
}

#[tokio::test]
async fn test_p3_prototype_permission_policy() {
    let temp_ws = tempfile::tempdir().unwrap();
    let temp_session = tempfile::tempdir().unwrap();

    let policy = PrototypePermissionPolicy::new(temp_ws.path(), temp_session.path());

    // 1. Workspace paths are allowed
    let valid_file = temp_ws.path().join("src/main.rs");
    assert!(policy.check_file_read(&valid_file).is_ok());
    assert!(policy.check_file_write(&valid_file).is_ok());
    assert!(policy.check_command_work_dir(temp_ws.path()).is_ok());

    // 2. Session tmp dir paths are allowed
    let session_file = temp_session.path().join("temp_out.txt");
    assert!(policy.check_file_read(&session_file).is_ok());
    assert!(policy.check_file_write(&session_file).is_ok());
    assert!(policy.check_command_work_dir(temp_session.path()).is_ok());

    // 3. Paths outside workspace and session tmp are rejected
    let outside_path = PathBuf::from("/etc/passwd");
    assert!(policy.check_file_read(&outside_path).is_err());
    assert!(policy.check_file_write(&outside_path).is_err());
    assert!(policy.check_command_work_dir(&outside_path).is_err());

    // 4. HTTP is rejected in Prototype
    assert!(policy.check_http().is_err());
}

#[tokio::test]
async fn test_p3_hook_manager_enforces_permission_on_file_editor() {
    let temp_ws = tempfile::tempdir().unwrap();
    let temp_session = tempfile::tempdir().unwrap();

    let preopens = vec![
        temp_ws.path().to_path_buf(),
        temp_session.path().to_path_buf(),
    ];
    let plugin = WasmPlugin::load_from_file_with_dirs(
        "file_editor",
        &file_editor_component_path(),
        &preopens,
    )
    .await
    .unwrap();

    let policy = PrototypePermissionPolicy::new(temp_ws.path(), temp_session.path());
    let mut manager = HookManager::empty().with_permission_policy(policy);
    manager.add_plugin(plugin).unwrap();
    manager.initialize().await.unwrap();

    // 1. Writing to a file within workspace succeeds
    let target_file = temp_ws.path().join("test_write.txt");
    let allowed_call = manager
        .action(
            HOOK_TOOLS_CALL,
            "file_editor",
            None,
            serde_json::to_value(ToolCallRequest {
                call_id: "call_wf_ok".into(),
                tool_id: "tool_wf".into(),
                name: "write_file".into(),
                arguments: serde_json::json!({
                    "path": target_file.to_str().unwrap(),
                    "content": "workspace content"
                }),
            })
            .unwrap(),
        )
        .await
        .unwrap();

    let res: ToolResult = serde_json::from_value(allowed_call).unwrap();
    assert!(res.success);
    assert_eq!(
        std::fs::read_to_string(&target_file).unwrap(),
        "workspace content"
    );

    // 2. Writing to an outside path is blocked by PermissionPolicy
    let blocked_call = manager
        .action(
            HOOK_TOOLS_CALL,
            "file_editor",
            None,
            serde_json::to_value(ToolCallRequest {
                call_id: "call_wf_blocked".into(),
                tool_id: "tool_wf".into(),
                name: "write_file".into(),
                arguments: serde_json::json!({
                    "path": "/tmp/forbidden_outside_write.txt",
                    "content": "hacked"
                }),
            })
            .unwrap(),
        )
        .await
        .unwrap();

    let res_blocked: ToolResult = serde_json::from_value(blocked_call).unwrap();
    assert!(!res_blocked.success);
    assert!(res_blocked
        .output
        .to_display_string()
        .contains("Permission denied"));
    assert!(!Path::new("/tmp/forbidden_outside_write.txt").exists());

    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_p3_react_loop_with_tool_call_and_commit() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    // Turn 1: Model returns FunctionCall for shell
    let fc_item = Item::FunctionCall {
        id: Some("fc_1".into()),
        call_id: "call_react_1".into(),
        name: "shell".into(),
        arguments: r#"{"command":"printf 'tool execution success'"}"#.into(),
        status: None,
    };
    let resp1 = make_json_response("resp_1", vec![fc_item], None);

    // Turn 2: Model receives ToolOutput and returns final message
    let final_msg = Item::assistant_message("Here is the final answer after running the tool.");
    let resp2 = make_json_response("resp_2", vec![final_msg], None);

    let (server_url, server_handle) = start_mock_server(vec![resp1, resp2]).await;

    let store = SqliteControlStore::open(&store_path).unwrap();
    let ws = WorkspaceSpec::new(&workspace_path).unwrap();
    store.ensure_workspace(&ws).unwrap();

    // Create ConfigRevision with shell extension enabled
    let cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("test-model".into()),
            ..Default::default()
        },
        vec![ExtensionConfigItem {
            name: "shell".into(),
            path: shell_component_path().to_string_lossy().to_string(),
            enabled: true,
            config: serde_json::json!({"default_timeout_seconds": 10}),
        }],
        ragent::config::ContextSummaryMode::default(),
    );
    let cfg_ref = store.ensure_config(&cfg).unwrap();

    let session_id = SessionId::generate();
    let spec = SessionSpec::new(session_id.clone(), "Basic prompt", cfg_ref, ws.id);
    store.create_session(&spec, None, None).unwrap();

    let config = AgentConfig::new(&server_url, "fake_key", "test-model");
    let service = ControlService::new(store, config);

    let cancel = CancellationToken::new();
    let result = service
        .run_session(&session_id, "Please run tool", cancel, None)
        .await
        .unwrap();

    assert_eq!(
        result.final_text,
        "Here is the final answer after running the tool."
    );

    // Verify all 4 items in session context:
    // 1. Input message
    // 2. ModelOutput FunctionCall
    // 3. ToolOutput FunctionCallOutput
    // 4. ModelOutput Assistant Message
    let items = service.read_context(&session_id).unwrap();
    assert_eq!(items.len(), 4);

    assert!(matches!(items[0], Item::Message { .. }));
    assert!(
        matches!(items[1], Item::FunctionCall { ref call_id, .. } if call_id == "call_react_1")
    );
    assert!(
        matches!(items[2], Item::FunctionCallOutput { ref call_id, .. } if call_id == "call_react_1")
    );
    assert!(matches!(items[3], Item::Message { .. }));

    // Verify batches: Input #0, ModelOutput #1, ToolOutput #2, ModelOutput #3
    let batches = service.store().read_batches(&session_id).unwrap();
    assert_eq!(batches.len(), 4);

    // Verify events: SessionCreated, ActivationRequested, ActivationStarted, TurnStarted, TurnCompleted, ToolCallStarted, ToolCallFinished, TurnStarted, TurnCompleted, ActivationCompleted
    let events = service.read_events(&session_id).unwrap();
    let event_kinds: Vec<&str> = events.iter().map(|e| e.event.kind_str()).collect();
    assert!(event_kinds.contains(&"session_created"));
    assert!(event_kinds.contains(&"activation_requested"));
    assert!(event_kinds.contains(&"tool_call_started"));
    assert!(event_kinds.contains(&"tool_call_finished"));
    assert!(event_kinds.contains(&"activation_completed"));

    server_handle.abort();
}
