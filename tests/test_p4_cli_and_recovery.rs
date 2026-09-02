use openresponses_rust::{Item, Usage};
use ragent::cli::run_cli;
use ragent::domain::session::SessionPhase;
use ragent::store::sqlite::SqliteControlStore;
use ragent::AgentConfig;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

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
async fn test_p4_cli_lifecycle_and_restart_continuity() {
    let temp = tempfile::tempdir().unwrap();
    let store_dir = temp.path().join("store");
    let store_file = store_dir.join("control.sqlite3");
    let ws_dir = temp.path().join("ws");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let resp1 = make_json_response(
        "resp_1",
        vec![Item::assistant_message("CLI Response 1")],
        None,
    );
    let resp2 = make_json_response(
        "resp_2",
        vec![Item::assistant_message("CLI Response 2")],
        None,
    );

    let (server_url, server_handle) = start_mock_server(vec![resp1, resp2]).await;

    let config = AgentConfig::new(&server_url, "fake_token", "test-model");

    // 1. CLI: ragent session create --workspace <ws_dir>
    let args_create = vec![
        "ragent".into(),
        "session".into(),
        "create".into(),
        "--workspace".into(),
        ws_dir.to_str().unwrap().into(),
        "--dir".into(),
        store_file.to_str().unwrap().into(),
    ];
    run_cli(&args_create, config.clone()).await.unwrap();

    // Verify session in sqlite
    let store = SqliteControlStore::open(&store_file).unwrap();
    let list = store.list_sessions().unwrap();
    assert_eq!(list.len(), 1);
    let session_id = list[0].0.id.clone();
    drop(store);

    // 2. CLI: ragent session list
    let args_list = vec![
        "ragent".into(),
        "session".into(),
        "list".into(),
        "--dir".into(),
        store_file.to_str().unwrap().into(),
    ];
    run_cli(&args_list, config.clone()).await.unwrap();

    // 3. CLI: ragent session run <session_id> "First user prompt"
    let args_run1 = vec![
        "ragent".into(),
        "session".into(),
        "run".into(),
        session_id.to_string(),
        "First user prompt".into(),
        "--dir".into(),
        store_file.to_str().unwrap().into(),
    ];
    run_cli(&args_run1, config.clone()).await.unwrap();

    // Verify status after turn 1
    let store = SqliteControlStore::open(&store_file).unwrap();
    let status1 = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(status1.phase, SessionPhase::Open);
    assert_eq!(status1.local_item_count, 2);
    assert_eq!(status1.batch_count, 2);
    drop(store);

    // 4. CLI: ragent session show <session_id>
    let args_show = vec![
        "ragent".into(),
        "session".into(),
        "show".into(),
        session_id.to_string(),
        "--dir".into(),
        store_file.to_str().unwrap().into(),
    ];
    run_cli(&args_show, config.clone()).await.unwrap();

    // 5. CLI: ragent session history <session_id>
    let args_hist = vec![
        "ragent".into(),
        "session".into(),
        "history".into(),
        session_id.to_string(),
        "--dir".into(),
        store_file.to_str().unwrap().into(),
    ];
    run_cli(&args_hist, config.clone()).await.unwrap();

    // 6. Simulate Process Exit & Restart, then Run Turn 2
    let args_run2 = vec![
        "ragent".into(),
        "session".into(),
        "run".into(),
        session_id.to_string(),
        "Second user prompt".into(),
        "--dir".into(),
        store_file.to_str().unwrap().into(),
    ];
    run_cli(&args_run2, config.clone()).await.unwrap();

    // Verify status after turn 2
    let store = SqliteControlStore::open(&store_file).unwrap();
    let status2 = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(status2.local_item_count, 4);
    assert_eq!(status2.batch_count, 4);

    let items = store.read_local_items(&session_id).unwrap();
    assert_eq!(items.len(), 4);

    // 7. Verify NO old .ragent/sessions/*.json files exist anywhere
    assert!(!ws_dir.join(".ragent/sessions").exists());
    assert!(!temp.path().join(".ragent/sessions").exists());

    server_handle.abort();
}

#[tokio::test]
async fn test_p4_recovery_rebuilds_wiped_projection_and_continues() {
    let temp = tempfile::tempdir().unwrap();
    let store_file = temp.path().join("control.sqlite3");
    let ws_dir = temp.path().join("ws");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let resp1 = make_json_response(
        "resp_1",
        vec![Item::assistant_message("Initial response")],
        None,
    );
    let resp2 = make_json_response(
        "resp_2",
        vec![Item::assistant_message("Response after recovery")],
        None,
    );

    let (server_url, server_handle) = start_mock_server(vec![resp1, resp2]).await;
    let config = AgentConfig::new(&server_url, "fake_token", "test-model");

    // Create session and run turn 1
    let store = SqliteControlStore::open(&store_file).unwrap();
    let service = ragent::control::service::ControlService::new(store, config.clone());
    let spec = service.create_session(&ws_dir, None, None).unwrap();
    let session_id = spec.id.clone();

    let cancel = tokio_util::sync::CancellationToken::new();
    service
        .run_session(&session_id, "Hello before wipe", cancel, None)
        .await
        .unwrap();

    // Intentionally wipe the session_status table to simulate lagging/corrupted projection table
    {
        let conn = rusqlite::Connection::open(&store_file).unwrap();
        conn.execute("DELETE FROM session_status", []).unwrap();
    }

    // 1. Reopen store and verify list_sessions automatically rebuilds missing status without returning fake initial
    let store_reopened = SqliteControlStore::open(&store_file).unwrap();
    let service2 = ragent::control::service::ControlService::new(store_reopened, config.clone());
    let list = service2.list_sessions().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].1.phase, SessionPhase::Open);
    assert_eq!(list[0].1.local_item_count, 2);
    assert_eq!(list[0].1.batch_count, 2);

    // 2. Wipe status table AGAIN
    {
        let conn = rusqlite::Connection::open(&store_file).unwrap();
        conn.execute("DELETE FROM session_status", []).unwrap();
    }

    // 3. Directly run session run WITHOUT any prior get_session or list_sessions calls!
    let store_reopened3 = SqliteControlStore::open(&store_file).unwrap();
    let service3 = ragent::control::service::ControlService::new(store_reopened3, config);
    let cancel2 = tokio_util::sync::CancellationToken::new();
    let res = service3
        .run_session(&session_id, "Hello after wipe directly", cancel2, None)
        .await
        .unwrap();

    assert_eq!(res.final_text, "Response after recovery");
    assert_eq!(res.status.local_item_count, 4);
    assert_eq!(res.status.batch_count, 4);

    server_handle.abort();
}
