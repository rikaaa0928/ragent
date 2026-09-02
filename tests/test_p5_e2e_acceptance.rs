mod common;

use common::shell_component_path;
use openresponses_rust::{Item, MessageContent, Usage};
use ragent::control::service::ControlService;
use ragent::domain::config::{ConfigRevision, ExtensionConfigItem};
use ragent::domain::ids::SessionId;
use ragent::domain::session::{SessionPhase, SessionSpec};
use ragent::domain::workspace::WorkspaceSpec;
use ragent::store::sqlite::SqliteControlStore;
use ragent::AgentConfig;
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
async fn test_p5_full_e2e_acceptance_10_steps() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    // Step 3 output: Reasoning Item + FunctionCall Item
    let reasoning_item = Item::Reasoning {
        id: Some("rs_e2e_1".into()),
        status: None,
        content: None,
        summary: vec![MessageContent::SummaryText {
            text: "I need to run a shell command to list the workspace files.".into(),
        }],
        encrypted_content: Some("enc_sig_e2e_1".into()),
    };
    let function_call_item = Item::FunctionCall {
        id: Some("fc_e2e_1".into()),
        call_id: "call_e2e_shell_1".into(),
        name: "shell".into(),
        arguments: r#"{"command":"printf 'file_a.txt\nfile_b.txt'"}"#.into(),
        status: None,
    };
    let resp1 = make_json_response(
        "resp_e2e_step1",
        vec![reasoning_item, function_call_item],
        None,
    );

    // Step 6 output: Final assistant message
    let final_assistant_item = Item::assistant_message("Found files: file_a.txt, file_b.txt.");
    let resp2 = make_json_response("resp_e2e_step2", vec![final_assistant_item], None);

    // Step 10 output: Second run assistant message
    let second_run_assistant_item = Item::assistant_message("Second run completed successfully.");
    let resp3 = make_json_response("resp_e2e_step3", vec![second_run_assistant_item], None);

    let (server_url, server_handle) = start_mock_server(vec![resp1, resp2, resp3]).await;
    let config = AgentConfig::new(&server_url, "token_e2e", "test-model");

    // =========================================================================
    // Step 1: 在临时 Store 创建 Workspace、Config 和 Session
    // =========================================================================
    let store = SqliteControlStore::open(&store_path).unwrap();
    let ws = WorkspaceSpec::new(&workspace_path).unwrap();
    store.ensure_workspace(&ws).unwrap();

    let cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("test-model".into()),
            ..Default::default()
        },
        vec![ExtensionConfigItem {
            name: "shell".into(),
            path: shell_component_path().to_string_lossy().to_string(),
            enabled: true,
            config: serde_json::json!({"default_timeout_seconds": 15}),
        }],
        ragent::config::ContextSummaryMode::default(),
    );
    let cfg_ref = store.ensure_config(&cfg).unwrap();

    let session_id = SessionId::generate();
    let spec = SessionSpec::new(session_id.clone(), "E2E Basic prompt", cfg_ref, ws.id);
    store.create_session(&spec, None, None).unwrap();

    let service = ControlService::new(store, config.clone());

    // =========================================================================
    // Step 2-6:
    // 2. 提交用户 Message Item
    // 3. 模型返回 Reasoning 和 FunctionCall
    // 4. Extension 在 Workspace 内执行工具
    // 5. ToolOutput 以 FunctionCallOutput Item 提交
    // 6. 模型返回最终 Message
    // =========================================================================
    let cancel = CancellationToken::new();
    let run1_result = service
        .run_session(&session_id, "Please list files", cancel, None)
        .await
        .unwrap();

    assert_eq!(
        run1_result.final_text,
        "Found files: file_a.txt, file_b.txt."
    );
    assert_eq!(run1_result.status.phase, SessionPhase::Open);
    assert!(run1_result.status.active_activation_id.is_none());
    assert_eq!(run1_result.status.batch_count, 4); // Input #0, Model #1, Tool #2, Model #3
    assert_eq!(run1_result.status.local_item_count, 5); // User(1) + Reasoning(1)+FC(1) + ToolOutput(1) + Assistant(1) = 5 items

    let items_round1 = service.read_context(&session_id).unwrap();
    assert_eq!(items_round1.len(), 5);
    assert!(matches!(items_round1[0], Item::Message { .. }));
    assert!(matches!(items_round1[1], Item::Reasoning { .. }));
    assert!(
        matches!(items_round1[2], Item::FunctionCall { ref call_id, .. } if call_id == "call_e2e_shell_1")
    );
    assert!(
        matches!(items_round1[3], Item::FunctionCallOutput { ref call_id, .. } if call_id == "call_e2e_shell_1")
    );
    assert!(matches!(items_round1[4], Item::Message { .. }));

    // =========================================================================
    // Step 7: 退出 CLI 进程 (drop service and connection)
    // =========================================================================
    drop(service);

    // =========================================================================
    // Step 8: 清空或故意落后 session_status
    // =========================================================================
    {
        let conn = rusqlite::Connection::open(&store_path).unwrap();
        conn.execute("DELETE FROM session_status", []).unwrap();
    }

    // =========================================================================
    // Step 9: 重新运行 CLI / Service，得到相同历史和状态
    // =========================================================================
    let store_reopened = SqliteControlStore::open(&store_path).unwrap();
    let service_reopened = ControlService::new(store_reopened, config);

    let (_spec_reopened, status_reopened) =
        service_reopened.get_session(&session_id).unwrap().unwrap();
    assert_eq!(status_reopened.phase, SessionPhase::Open);
    assert_eq!(status_reopened.local_item_count, 5);
    assert_eq!(status_reopened.batch_count, 4);
    assert!(status_reopened.active_activation_id.is_none());

    let items_reopened = service_reopened.read_context(&session_id).unwrap();
    assert_eq!(items_reopened, items_round1);

    // =========================================================================
    // Step 10: 再提交一轮输入，确认只追加新 Batch
    // =========================================================================
    let cancel2 = CancellationToken::new();
    let run2_result = service_reopened
        .run_session(&session_id, "Second prompt after restart", cancel2, None)
        .await
        .unwrap();

    assert_eq!(run2_result.final_text, "Second run completed successfully.");
    assert_eq!(run2_result.status.batch_count, 6); // + Input #4, Model #5
    assert_eq!(run2_result.status.local_item_count, 7); // + User(1), Assistant(1)

    let all_batches = service_reopened.store().read_batches(&session_id).unwrap();
    assert_eq!(all_batches.len(), 6);
    for (i, b) in all_batches.iter().enumerate() {
        assert_eq!(b.batch_seq.as_u64(), i as u64);
    }

    let all_events = service_reopened.read_events(&session_id).unwrap();
    for (i, e) in all_events.iter().enumerate() {
        assert_eq!(e.event_seq.as_u64(), i as u64);
    }

    server_handle.abort();
}
