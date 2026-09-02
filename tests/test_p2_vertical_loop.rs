use openresponses_rust::{Item, Usage};
use ragent::control::service::ControlService;
use ragent::core::agent::AgentCore;
use ragent::core::context::ContextProjection;
use ragent::core::model::ModelClient;
use ragent::domain::event::SessionEvent;
use ragent::domain::ids::{ActivationId, SessionId};
use ragent::domain::workspace::WorkspaceSpec;
use ragent::store::sqlite::SqliteControlStore;
use ragent::{AgentConfig, ContextSummaryMode};
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

async fn start_mock_open_responses_server(
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
async fn test_p2_agent_core_step_pure() {
    let usage = Usage {
        input_tokens: 12,
        output_tokens: 5,
        total_tokens: 17,
        input_tokens_details: openresponses_rust::InputTokensDetails { cached_tokens: 0 },
        output_tokens_details: openresponses_rust::OutputTokensDetails {
            reasoning_tokens: 0,
        },
    };
    let mock_resp = make_json_response(
        "resp_pure_step",
        vec![Item::assistant_message("Pure step result")],
        Some(usage),
    );

    let (server_url, server_handle) = start_mock_open_responses_server(vec![mock_resp]).await;
    let client = ModelClient::new(&server_url, "fake_key");

    let projection = ContextProjection::new(vec![Item::user_message("Hello from user")]);
    let step = AgentCore::step(
        "You are a helpful assistant",
        &projection,
        "test-model",
        Some(0.7),
        Some(1000),
        None,
        vec![],
        ContextSummaryMode::Off,
        &client,
    )
    .await
    .unwrap();

    assert_eq!(step.text, "Pure step result");
    assert_eq!(step.response_id.as_deref(), Some("resp_pure_step"));
    assert_eq!(step.function_calls.len(), 0);
    assert_eq!(step.output_items.len(), 1);
    assert_eq!(step.usage.as_ref().unwrap().total_tokens, 17);

    server_handle.abort();
}

#[tokio::test]
async fn test_p2_vertical_loop_no_tool_two_turns() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let usage1 = Usage {
        input_tokens: 10,
        output_tokens: 5,
        total_tokens: 15,
        input_tokens_details: openresponses_rust::InputTokensDetails { cached_tokens: 0 },
        output_tokens_details: openresponses_rust::OutputTokensDetails {
            reasoning_tokens: 0,
        },
    };
    let resp1 = make_json_response(
        "resp_turn_1",
        vec![Item::assistant_message("Answer 1")],
        Some(usage1),
    );

    let usage2 = Usage {
        input_tokens: 25,
        output_tokens: 10,
        total_tokens: 35,
        input_tokens_details: openresponses_rust::InputTokensDetails { cached_tokens: 0 },
        output_tokens_details: openresponses_rust::OutputTokensDetails {
            reasoning_tokens: 0,
        },
    };
    let resp2 = make_json_response(
        "resp_turn_2",
        vec![Item::assistant_message("Answer 2")],
        Some(usage2),
    );

    let (server_url, server_handle) = start_mock_open_responses_server(vec![resp1, resp2]).await;

    let config = AgentConfig::new(&server_url, "fake_key", "test-model");
    let service = ControlService::open(&store_path, config).unwrap();

    // 1. Create Session
    let spec = service
        .create_session(&workspace_path, Some("Basic prompt"), None)
        .unwrap();
    let session_id = spec.id.clone();

    // 2. First turn
    let cancel1 = CancellationToken::new();
    let res1 = service
        .run_session(&session_id, "User query 1", cancel1, None)
        .await
        .unwrap();

    assert_eq!(res1.final_text, "Answer 1");
    assert_eq!(res1.items.len(), 2); // 1 input + 1 model output = 2
    assert_eq!(res1.status.local_item_count, 2);
    assert_eq!(res1.status.batch_count, 2); // Input batch #0, ModelOutput batch #1
    assert!(res1.status.active_activation_id.is_none());

    // 3. Second turn in same session
    let cancel2 = CancellationToken::new();
    let res2 = service
        .run_session(&session_id, "User query 2", cancel2, None)
        .await
        .unwrap();

    assert_eq!(res2.final_text, "Answer 2");
    // Context now contains all 4 items
    assert_eq!(res2.items.len(), 4);
    assert_eq!(res2.status.local_item_count, 4);
    assert_eq!(res2.status.batch_count, 4); // Input #0, Model #1, Input #2, Model #3
    assert!(res2.status.active_activation_id.is_none());

    // Verify batches in store are ordered and contiguous
    let batches = service.store().read_batches(&session_id).unwrap();
    assert_eq!(batches.len(), 4);
    assert_eq!(batches[0].batch_seq.as_u64(), 0);
    assert_eq!(batches[1].batch_seq.as_u64(), 1);
    assert_eq!(batches[2].batch_seq.as_u64(), 2);
    assert_eq!(batches[3].batch_seq.as_u64(), 3);

    server_handle.abort();
}

#[tokio::test]
async fn test_p2_interrupted_activation_on_restart() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let session_id = SessionId::generate();
    let act_id = ActivationId::generate();

    // Open store, create session, and simulate an active activation where process died before completion
    {
        let store = SqliteControlStore::open(&store_path).unwrap();
        let ws = WorkspaceSpec::new(&workspace_path).unwrap();
        store.ensure_workspace(&ws).unwrap();
        let cfg = ragent::domain::config::ConfigRevision::new(
            openresponses_rust::CreateResponseBody {
                model: Some("test-model".into()),
                ..Default::default()
            },
            vec![],
            ragent::config::ContextSummaryMode::default(),
        );
        store.ensure_config(&cfg).unwrap();

        let spec = ragent::domain::session::SessionSpec::new(
            session_id.clone(),
            "Prompt",
            cfg.config_ref.clone(),
            ws.id,
        );
        store.create_session(&spec, None, None).unwrap();

        // Commit input -> sets active_activation_id = Some(act_id)
        store
            .commit_input(
                &session_id,
                &act_id,
                &cfg.config_ref,
                vec![Item::user_message("Hello")],
                None,
                None,
            )
            .unwrap();

        let status = store.get_status(&session_id).unwrap().unwrap();
        assert_eq!(status.active_activation_id.as_ref(), Some(&act_id));
    } // Process exits without completing activation

    // Restart process: open store afresh for queries (read-only query does NOT write interruption)
    let store2 = SqliteControlStore::open(&store_path).unwrap();
    let query_status = store2.get_status(&session_id).unwrap().unwrap();
    assert_eq!(query_status.active_activation_id.as_ref(), Some(&act_id));

    // Now start mock server and run the session again
    let mock_resp = make_json_response(
        "resp_recovered_turn",
        vec![Item::assistant_message("Recovered answer")],
        None,
    );
    let (server_url, server_handle) = start_mock_open_responses_server(vec![mock_resp]).await;
    let config = AgentConfig::new(&server_url, "token", "test-model");

    let service = ControlService::new(store2, config);
    let cancel = tokio_util::sync::CancellationToken::new();

    // Session runner recovers the interrupted activation under exclusive lock and runs new turn
    let run_res = service
        .run_session(&session_id, "New turn after restart", cancel, None)
        .await
        .unwrap();

    assert_eq!(run_res.final_text, "Recovered answer");
    assert!(run_res.status.active_activation_id.is_none());

    // Verify ActivationInterrupted event was recorded in events prior to new activation
    let events = service.read_events(&session_id).unwrap();
    let has_interrupted = events
        .iter()
        .any(|e| matches!(e.event, SessionEvent::ActivationInterrupted { .. }));
    assert!(has_interrupted);

    server_handle.abort();
}
