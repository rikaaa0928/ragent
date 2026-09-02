mod common;

use openresponses_rust::{Item, Usage};
use ragent::control::lock::SessionLock;
use ragent::control::service::ControlService;
use ragent::domain::config::{ConfigRevision, ExtensionConfigItem};
use ragent::domain::event::SessionEvent;
use ragent::domain::ids::{ActivationId, SessionId};
use ragent::domain::session::SessionSpec;
use ragent::domain::workspace::WorkspaceSpec;
use ragent::store::sqlite::SqliteControlStore;
use ragent::AgentConfig;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

fn make_json_response(
    id: &str,
    model: &str,
    output: Vec<Item>,
    usage: Option<Usage>,
) -> serde_json::Value {
    let output_val = serde_json::to_value(output).unwrap();
    let usage_val = usage
        .map(|u| serde_json::to_value(u).unwrap())
        .unwrap_or(serde_json::Value::Null);

    serde_json::json!({
        "id": id,
        "object": "response",
        "created_at": 1700000000,
        "status": "completed",
        "model": model,
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

async fn start_mock_server_inspect(
    responses: Vec<serde_json::Value>,
    received_models: Arc<std::sync::Mutex<Vec<String>>>,
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
            let models_collector = Arc::clone(&received_models);

            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let req_text = String::from_utf8_lossy(&buf[..n]);

                // Extract body and model name from HTTP request
                if let Some(body_start) = req_text.find("\r\n\r\n") {
                    let body_str = &req_text[body_start + 4..];
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(body_str) {
                        if let Some(m) = val.get("model").and_then(|v| v.as_str()) {
                            models_collector.lock().unwrap().push(m.to_string());
                        }
                    }
                }

                let idx = idx_atomic.fetch_add(1, Ordering::SeqCst);
                let resp_val = if idx < responses.len() {
                    responses[idx].clone()
                } else {
                    responses.last().cloned().unwrap_or_else(|| {
                        make_json_response("resp_fallback", "model", vec![], None)
                    })
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

// =========================================================================
// P1 Test 1: Extension path resolution & Missing extension explicit error
// =========================================================================
#[tokio::test]
async fn test_missing_extension_explicit_error() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let store = SqliteControlStore::open(&store_path).unwrap();
    let ws = WorkspaceSpec::new(&workspace_path).unwrap();
    store.ensure_workspace(&ws).unwrap();

    // ConfigRevision with a non-existent extension path
    let cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("test-model".into()),
            ..Default::default()
        },
        vec![ExtensionConfigItem {
            name: "non_existent_tool".into(),
            path: "/path/to/definitely/non_existent_extension.wasm".into(),
            enabled: true,
            config: serde_json::json!({}),
        }],
        ragent::config::ContextSummaryMode::default(),
    );
    let cfg_ref = store.ensure_config(&cfg).unwrap();

    let session_id = SessionId::generate();
    let spec = SessionSpec::new(session_id.clone(), "Prompt", cfg_ref, ws.id);
    store.create_session(&spec, None, None).unwrap();

    let config = AgentConfig::new("http://127.0.0.1:12345", "token", "test-model");
    let service = ControlService::new(store, config);
    let cancel = CancellationToken::new();

    let err = service
        .run_session(&session_id, "Hello", cancel, None)
        .await
        .unwrap_err();

    let err_str = err.to_string();
    assert!(
        err_str.contains("non_existent_tool") && err_str.contains("does not exist"),
        "Error should explicitly mention missing extension: {}",
        err_str
    );
}

// =========================================================================
// P1 Test 2: ConfigRevision frozen execution parameters
// =========================================================================
#[tokio::test]
async fn test_config_revision_controls_execution_model_frozen() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let models_called = Arc::new(std::sync::Mutex::new(Vec::new()));
    let resp = make_json_response(
        "resp_frozen",
        "frozen-model-gpt-4o",
        vec![Item::assistant_message("Hello from frozen model")],
        None,
    );
    let (server_url, server_handle) =
        start_mock_server_inspect(vec![resp], Arc::clone(&models_called)).await;

    let store = SqliteControlStore::open(&store_path).unwrap();
    let ws = WorkspaceSpec::new(&workspace_path).unwrap();
    store.ensure_workspace(&ws).unwrap();

    // Create session frozen to "frozen-model-gpt-4o"
    let cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("frozen-model-gpt-4o".into()),
            temperature: Some(0.42),
            ..Default::default()
        },
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );
    let cfg_ref = store.ensure_config(&cfg).unwrap();

    let session_id = SessionId::generate();
    let spec = SessionSpec::new(session_id.clone(), "Prompt", cfg_ref, ws.id);
    store.create_session(&spec, None, None).unwrap();

    // Create Service with DIFFERENT process model "claude-3-5-sonnet"
    let process_config = AgentConfig::new(&server_url, "token", "claude-3-5-sonnet");
    let service = ControlService::new(store, process_config);
    let cancel = CancellationToken::new();

    let res = service
        .run_session(&session_id, "Hello", cancel, None)
        .await
        .unwrap();

    assert_eq!(res.final_text, "Hello from frozen model");

    // Verify the HTTP request sent to LLM used the model frozen in ConfigRevision ("frozen-model-gpt-4o")
    let calls = models_called.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], "frozen-model-gpt-4o");

    server_handle.abort();
}

// =========================================================================
// P1 Test 3: Session Lock prevents concurrent runs & queries do not disrupt
// =========================================================================
#[tokio::test]
async fn test_session_lock_and_query_safety() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let store = SqliteControlStore::open(&store_path).unwrap();
    let ws = WorkspaceSpec::new(&workspace_path).unwrap();
    store.ensure_workspace(&ws).unwrap();
    let cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("test-model".into()),
            ..Default::default()
        },
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );
    let cfg_ref = store.ensure_config(&cfg).unwrap();

    let session_id = SessionId::generate();
    let spec = SessionSpec::new(session_id.clone(), "Prompt", cfg_ref, ws.id);
    store.create_session(&spec, None, None).unwrap();

    // 1. Acquire SessionLock
    let lock_guard = SessionLock::acquire(store.lock_dir().as_deref(), &session_id).unwrap();

    // 2. Another attempt to acquire lock on same session must fail
    let second_lock = SessionLock::acquire(store.lock_dir().as_deref(), &session_id);
    assert!(second_lock.is_err());

    // 3. Opening store afresh for query commands while session is locked/running must succeed and NOT interrupt
    let store_query = SqliteControlStore::open(&store_path).unwrap();
    let list = store_query.list_sessions().unwrap();
    assert_eq!(list.len(), 1);

    drop(lock_guard);

    // 4. After drop, lock can be acquired again
    let lock_again = SessionLock::acquire(store.lock_dir().as_deref(), &session_id);
    assert!(lock_again.is_ok());
}

// =========================================================================
// P1 Test 4: Guaranteed terminal event (ActivationFailed) on mid-run failure
// =========================================================================
#[tokio::test]
async fn test_activation_terminal_event_guarantee_on_error() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    // Point to non-existent server port to cause a network error during step
    let config = AgentConfig::new("http://127.0.0.1:1", "token", "test-model");
    let service = ControlService::open(&store_path, config.clone()).unwrap();

    let spec = service.create_session(&workspace_path, None, None).unwrap();
    let session_id = spec.id.clone();

    let cancel = CancellationToken::new();
    let res = service
        .run_session(&session_id, "Hello error", cancel, None)
        .await;

    // Run must fail
    assert!(res.is_err());

    // Verify session status is NOT stuck with active_activation_id: Some(...)
    let store = service.store();
    let status = store.get_status(&session_id).unwrap().unwrap();
    assert!(
        status.active_activation_id.is_none(),
        "active_activation_id must be cleared on failure"
    );
    assert!(status.last_error.is_some());

    // Verify ActivationFailed event was recorded
    let events = store.read_events(&session_id).unwrap();
    let has_failed_event = events
        .iter()
        .any(|e| matches!(e.event, SessionEvent::ActivationFailed { .. }));
    assert!(
        has_failed_event,
        "ActivationFailed event must be recorded on error"
    );

    // Now start a working server and run a 2nd turn in the same session: must succeed without being blocked!
    let resp = make_json_response(
        "resp_ok",
        "test-model",
        vec![Item::assistant_message("Recovered from error")],
        None,
    );
    let (server_url, server_handle) =
        start_mock_server_inspect(vec![resp], Arc::new(std::sync::Mutex::new(Vec::new()))).await;

    let working_config = AgentConfig::new(&server_url, "token", "test-model");
    let working_service = ControlService::open(&store_path, working_config).unwrap();

    let cancel2 = CancellationToken::new();
    let res2 = working_service
        .run_session(&session_id, "Try again", cancel2, None)
        .await
        .unwrap();

    assert_eq!(res2.final_text, "Recovered from error");
    assert!(res2.status.active_activation_id.is_none());

    server_handle.abort();
}

// =========================================================================
// P1 Test 5: ensure_config rejects tampered hash and conflicting payload
// =========================================================================
#[test]
fn test_ensure_config_validates_hash_and_rejects_tampered_ref_or_collision() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteControlStore::open(temp.path().join("control.sqlite3")).unwrap();

    let template = openresponses_rust::CreateResponseBody {
        model: Some("model-a".into()),
        ..Default::default()
    };
    let mut cfg = ConfigRevision::new(
        template.clone(),
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );

    // 1. Valid config revision succeeds
    let ref1 = store.ensure_config(&cfg).unwrap();
    assert_eq!(ref1, cfg.config_ref);

    // 2. Tampering declared config_ref must be rejected
    cfg.config_ref = ragent::domain::ids::ConfigRef::new(
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    );
    let err_mismatch = store.ensure_config(&cfg).unwrap_err();
    assert!(err_mismatch.to_string().contains("ConfigRef mismatch"));

    // 3. Modifying payload while keeping original config_ref must also be rejected
    let mut tampered_payload = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("model-DIFFERENT".into()),
            ..Default::default()
        },
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );
    tampered_payload.config_ref = ref1.clone();
    let err_collision = store.ensure_config(&tampered_payload).unwrap_err();
    assert!(err_collision.to_string().contains("ConfigRef mismatch"));
}

// =========================================================================
// P1 Test 6: Project config uses standard format (name/enabled/config)
// =========================================================================
#[test]
fn test_project_config_deep_merge_and_strict_validation() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let temp_global_dir = tempfile::tempdir().unwrap();
    let config_dir = temp_global_dir.path();
    let extensions_dir = config_dir.join("extensions");
    let _ = std::fs::create_dir_all(&extensions_dir);
    let dummy_wasm = extensions_dir.join("dummy.wasm");
    std::fs::write(&dummy_wasm, b"dummy wasm").unwrap();

    let global_config = format!(
        r#"
[model]
name = "global-model"
temperature = 0.7

[[extensions]]
name = "dummy"
path = "{}"
enabled = true
[extensions.config]
key1 = "global_val"
"#,
        dummy_wasm.to_string_lossy()
    );
    std::fs::write(config_dir.join("config.toml"), global_config).unwrap();

    // Project config with standard name / enabled / config (NO path)
    let project_ragent_dir = workspace_path.join(".ragent");
    std::fs::create_dir_all(&project_ragent_dir).unwrap();
    let project_config_valid = r#"
[model]
name = "project-model"

[[extensions]]
name = "dummy"
enabled = false
[extensions.config]
key2 = "project_val"
"#;
    std::fs::write(project_ragent_dir.join("config.toml"), project_config_valid).unwrap();

    let store = SqliteControlStore::open(&store_path).unwrap();
    let service = ControlService::new(store, AgentConfig::new("http://127.0.0.1:0", "tok", "def"))
        .with_config_dir(config_dir);

    // Session creation succeeds and merges project config properly
    let spec = service.create_session(&workspace_path, None, None).unwrap();
    let config_rev = service
        .store()
        .get_config(&spec.default_config_ref)
        .unwrap()
        .unwrap();

    assert_eq!(
        config_rev.response_template.model.as_deref(),
        Some("project-model")
    );
    assert_eq!(config_rev.response_template.temperature, Some(0.7)); // inherited from global
    assert_eq!(config_rev.extensions.len(), 1);
    assert_eq!(config_rev.extensions[0].name, "dummy");
    assert!(!config_rev.extensions[0].enabled); // overridden by project
    assert_eq!(config_rev.extensions[0].config["key1"], "global_val"); // merged
    assert_eq!(config_rev.extensions[0].config["key2"], "project_val"); // merged

    // 2. Project config with unknown extension name must fail create_session explicitly
    let project_config_invalid = r#"
[[extensions]]
name = "unknown_ext_never_in_global"
enabled = true
"#;
    std::fs::write(
        project_ragent_dir.join("config.toml"),
        project_config_invalid,
    )
    .unwrap();
    let err = service
        .create_session(&workspace_path, None, None)
        .unwrap_err();
    assert!(err.to_string().contains("does not exist in global config"));

    // 3. Project config with disallowed 'path' field must fail create_session explicitly
    let project_config_disallowed_path = r#"
[[extensions]]
name = "dummy"
path = "/some/disallowed/path.wasm"
"#;
    std::fs::write(
        project_ragent_dir.join("config.toml"),
        project_config_disallowed_path,
    )
    .unwrap();
    let err2 = service
        .create_session(&workspace_path, None, None)
        .unwrap_err();
    assert!(err2
        .to_string()
        .contains("invalid extension configuration in project config"));
}

// =========================================================================
// P1 Test 7: Cancellation during long in-flight model request interrupts immediately
// =========================================================================
#[tokio::test]
async fn test_cancellation_during_long_model_request_interrupts_immediately() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_url = format!("http://127.0.0.1:{}", addr.port());

    // Server deliberately delays responding for 5 seconds
    let server_handle = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                let body = serde_json::to_string(&make_json_response(
                    "resp_slow",
                    "slow-model",
                    vec![Item::assistant_message("Delayed response")],
                    None,
                ))
                .unwrap();
                let http_response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(http_response.as_bytes()).await;
            });
        }
    });

    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("control.sqlite3");
    let workspace_path = temp.path().join("ws");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let config = AgentConfig::new(&server_url, "fake_tok", "test-model");
    let service = ControlService::open(&store_path, config).unwrap();
    let spec = service.create_session(&workspace_path, None, None).unwrap();
    let session_id = spec.id.clone();

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Trigger cancellation after 100ms while model request is pending
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        cancel_clone.cancel();
    });

    let start = std::time::Instant::now();
    let res = service
        .run_session(&session_id, "Hello prompt", cancel, None)
        .await;
    let elapsed = start.elapsed();

    // Must return within < 1 second (interrupted immediately, not waiting 5 seconds)
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "Cancellation took too long: {:?}",
        elapsed
    );
    assert!(matches!(res, Err(ragent::AgentError::Cancelled)));

    // Verify ActivationCancelled was recorded and status is clean
    let store = service.store();
    let events = store.read_events(&session_id).unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e.event, SessionEvent::ActivationCancelled)));

    let status = store.get_status(&session_id).unwrap().unwrap();
    assert!(status.active_activation_id.is_none());

    server_handle.abort();
}

// =========================================================================
// P1 Test 8: Store-level global runner lock rejects concurrent activations across different sessions
// =========================================================================
#[test]
fn test_store_level_global_runner_lock_rejects_different_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let lock_dir = temp.path().join("locks");

    let sess_a = SessionId::generate();
    let sess_b = SessionId::generate();

    // 1. Session A acquires global runner lock
    let lock_a = SessionLock::acquire(Some(&lock_dir), &sess_a).unwrap();

    // 2. Session B (different session ID!) attempts to acquire lock against same store and MUST fail
    let lock_b_err = SessionLock::acquire(Some(&lock_dir), &sess_b).unwrap_err();
    assert!(
        lock_b_err
            .to_string()
            .contains("Another activation is currently running in this store"),
        "Expected global runner lock collision error, got: {}",
        lock_b_err
    );

    // 3. Once Session A releases lock, Session B succeeds
    drop(lock_a);
    let lock_b = SessionLock::acquire(Some(&lock_dir), &sess_b).unwrap();
    assert_eq!(lock_b.session_id(), &sess_b);
}

// =========================================================================
// P1 Test 9: ConfigRevision rejects empty model name in ensure_config
// =========================================================================
#[test]
fn test_config_revision_rejects_empty_model() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteControlStore::open(temp.path().join("control.sqlite3")).unwrap();

    let invalid_cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: None, // Missing model
            ..Default::default()
        },
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );

    let err = store.ensure_config(&invalid_cfg).unwrap_err();
    assert!(err.to_string().contains("requires a non-empty model name"));

    let empty_name_cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("   ".into()), // Empty whitespace model
            ..Default::default()
        },
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );

    let err2 = store.ensure_config(&empty_name_cfg).unwrap_err();
    assert!(err2.to_string().contains("requires a non-empty model name"));
}

// =========================================================================
// P1 Test 10: Lagging projection is automatically detected and rebuilt from facts
// =========================================================================
#[test]
fn test_lagging_projection_rebuilds_automatically_on_query_and_commit() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("control.sqlite3");
    let store = SqliteControlStore::open(&db_path).unwrap();

    let ws = WorkspaceSpec::new(temp.path()).unwrap();
    store.ensure_workspace(&ws).unwrap();

    let cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("test-model".into()),
            ..Default::default()
        },
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );
    store.ensure_config(&cfg).unwrap();

    let session_id = SessionId::generate();
    let spec = SessionSpec::new(
        session_id.clone(),
        "Prompt",
        cfg.config_ref.clone(),
        ws.id.clone(),
    );
    store.create_session(&spec, None, None).unwrap();

    let act_id = ActivationId::generate();
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

    let status_before = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(status_before.local_item_count, 1);
    assert_eq!(status_before.batch_count, 1);
    assert_eq!(status_before.event_count, 2);

    // Intentionally corrupt/regress the session_status table to simulate a lagging projection
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let stale_status = ragent::domain::session::SessionStatus {
            projection_version: 1,
            phase: ragent::domain::session::SessionPhase::Open,
            active_activation_id: None,
            queued_activation_count: 0,
            local_item_count: 0, // Stale!
            effective_context_item_count: 0,
            batch_count: 0, // Stale!
            event_count: 1, // Stale!
            updated_at: chrono::Utc::now(),
            title: None,
            last_error: None,
        };
        let stale_json = serde_json::to_string(&stale_status).unwrap();
        conn.execute(
            "UPDATE session_status SET projected_through_event_seq = 1, projected_through_batch_seq = 0, payload_json = ?1 WHERE session_id = ?2",
            rusqlite::params![stale_json, session_id.as_str()],
        ).unwrap();
    }

    // Calling get_status detects that projected_through_event_seq (1) != event_count (2) and automatically rebuilds!
    let status_rebuilt = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(status_rebuilt.local_item_count, 1);
    assert_eq!(status_rebuilt.batch_count, 1);
    assert_eq!(status_rebuilt.event_count, 2);
    assert_eq!(status_rebuilt.active_activation_id, Some(act_id));

    // Calling list_sessions also reflects the accurate rebuilt projection
    let list = store.list_sessions().unwrap();
    assert_eq!(list[0].1.batch_count, 1);
    assert_eq!(list[0].1.local_item_count, 1);
}

// =========================================================================
// P1 Test 11: Corrupted/unparseable payload_json in session_status is automatically rebuilt from facts
// =========================================================================
#[test]
fn test_corrupted_status_payload_json_rebuilds_automatically() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("control.sqlite3");
    let store = SqliteControlStore::open(&db_path).unwrap();

    let ws = WorkspaceSpec::new(temp.path()).unwrap();
    store.ensure_workspace(&ws).unwrap();

    let cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("test-model".into()),
            ..Default::default()
        },
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );
    store.ensure_config(&cfg).unwrap();

    let session_id = SessionId::generate();
    let spec = SessionSpec::new(
        session_id.clone(),
        "Prompt",
        cfg.config_ref.clone(),
        ws.id.clone(),
    );
    store.create_session(&spec, None, None).unwrap();

    let act_id = ActivationId::generate();
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

    // Intentionally corrupt payload_json in session_status while keeping matching event/batch counts
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE session_status SET payload_json = '{ corrupted_invalid_json: true ' WHERE session_id = ?1",
            rusqlite::params![session_id.as_str()],
        ).unwrap();
    }

    // 1. get_status detects invalid JSON payload and automatically rebuilds from facts
    let status = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(status.phase, ragent::domain::session::SessionPhase::Open);
    assert_eq!(status.local_item_count, 1);
    assert_eq!(status.batch_count, 1);
    assert_eq!(status.event_count, 2);
    assert_eq!(status.active_activation_id, Some(act_id));

    // 2. Corrupt it again, and test list_sessions recovery
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE session_status SET payload_json = '{\"unknown_corrupted_field\": 123' WHERE session_id = ?1",
            rusqlite::params![session_id.as_str()],
        ).unwrap();
    }

    let list = store.list_sessions().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].1.batch_count, 1);
    assert_eq!(list[0].1.local_item_count, 1);
}

// =========================================================================
// P1 Test 12: Extension initialization and idempotent shutdown guarantees
// =========================================================================
#[tokio::test]
async fn test_hook_manager_initialize_and_shutdown_lifecycle_guarantees() {
    let shell_path = std::path::PathBuf::from(
        "extensions/shell/target/wasm32-wasip2/release/ragent_shell_extension.wasm",
    );
    let file_editor_path = std::path::PathBuf::from(
        "extensions/file_editor/target/wasm32-wasip2/release/ragent_file_editor_extension.wasm",
    );
    assert!(
        shell_path.exists() && file_editor_path.exists(),
        "WASM extension binaries not found. Please run ./scripts/build-extensions.sh first."
    );

    let plugin1 = ragent::hooks::runtime::WasmPlugin::load_from_file("shell", &shell_path)
        .await
        .unwrap();
    let plugin2 =
        ragent::hooks::runtime::WasmPlugin::load_from_file("file_editor", &file_editor_path)
            .await
            .unwrap();

    let mut manager = ragent::hooks::manager::HookManager::empty();
    manager
        .add_plugin_with_config(plugin1, serde_json::json!({}))
        .unwrap();
    manager
        .add_plugin_with_config(plugin2, serde_json::json!({}))
        .unwrap();

    // 1. Initialize succeeds
    manager.initialize().await.unwrap();

    // 2. Calling shutdown() shuts down initialized plugins
    manager.shutdown().await.unwrap();

    // 3. Calling shutdown() a second time is completely idempotent and safe (no double shutdown)
    manager.shutdown().await.unwrap();
}

// =========================================================================
// P1 Test 13: Shutdown fault injection ensures all plugins are cleaned up
// =========================================================================
#[tokio::test]
async fn test_shutdown_fault_injection_cleans_up_all_plugins() {
    let shell_path = std::path::PathBuf::from(
        "extensions/shell/target/wasm32-wasip2/release/ragent_shell_extension.wasm",
    );
    let file_editor_path = std::path::PathBuf::from(
        "extensions/file_editor/target/wasm32-wasip2/release/ragent_file_editor_extension.wasm",
    );
    let image_viewer_path = std::path::PathBuf::from(
        "extensions/image_viewer/target/wasm32-wasip2/release/ragent_image_viewer_extension.wasm",
    );
    assert!(
        shell_path.exists() && file_editor_path.exists() && image_viewer_path.exists(),
        "WASM extension binaries not found. Please run ./scripts/build-extensions.sh first."
    );

    let plugin1 = ragent::hooks::runtime::WasmPlugin::load_from_file("shell", &shell_path)
        .await
        .unwrap();
    let plugin2 =
        ragent::hooks::runtime::WasmPlugin::load_from_file("file_editor", &file_editor_path)
            .await
            .unwrap();
    let plugin3 =
        ragent::hooks::runtime::WasmPlugin::load_from_file("image_viewer", &image_viewer_path)
            .await
            .unwrap();

    // Inject simulated shutdown failure into plugin 2
    plugin2.set_simulate_shutdown_failure(true);

    let mut manager = ragent::hooks::manager::HookManager::empty();
    manager
        .add_plugin_with_config(plugin1, serde_json::json!({}))
        .unwrap();
    manager
        .add_plugin_with_config(plugin2, serde_json::json!({}))
        .unwrap();
    manager
        .add_plugin_with_config(plugin3, serde_json::json!({}))
        .unwrap();

    // Initialize all 3 plugins
    manager.initialize().await.unwrap();

    // Shutdown will encounter plugin2's failure, but MUST continue and shutdown plugin1 as well!
    let shutdown_err = manager.shutdown().await.unwrap_err();
    assert!(
        shutdown_err
            .to_string()
            .contains("injected shutdown failure"),
        "Expected injected shutdown failure error, got: {}",
        shutdown_err
    );

    // Verify all plugins were called for shutdown
    let plugins = manager.plugins();
    assert_eq!(
        plugins[0].shutdown_call_count(),
        1,
        "plugin1 must have shutdown called"
    );
    assert_eq!(
        plugins[1].shutdown_call_count(),
        1,
        "plugin2 must have shutdown called"
    );
    assert_eq!(
        plugins[2].shutdown_call_count(),
        1,
        "plugin3 must have shutdown called"
    );

    // Calling shutdown a second time is a safe no-op (no duplicate calls)
    manager.shutdown().await.unwrap();
    assert_eq!(plugins[0].shutdown_call_count(), 1);
    assert_eq!(plugins[1].shutdown_call_count(), 1);
    assert_eq!(plugins[2].shutdown_call_count(), 1);
}

// =========================================================================
// P1 Test 14: Normal commits advance sequences incrementally without false lagging replay
// =========================================================================
#[test]
fn test_normal_commits_advance_sequences_incrementally() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("control.sqlite3");
    let store = SqliteControlStore::open(&db_path).unwrap();

    // Initial rebuild_count is 0
    assert_eq!(store.rebuild_count(), 0);

    let ws = WorkspaceSpec::new(temp.path()).unwrap();
    store.ensure_workspace(&ws).unwrap();

    let cfg = ConfigRevision::new(
        openresponses_rust::CreateResponseBody {
            model: Some("test-model".into()),
            ..Default::default()
        },
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );
    store.ensure_config(&cfg).unwrap();

    let session_id = SessionId::generate();
    let spec = SessionSpec::new(
        session_id.clone(),
        "Prompt",
        cfg.config_ref.clone(),
        ws.id.clone(),
    );
    store.create_session(&spec, None, None).unwrap();
    assert_eq!(store.rebuild_count(), 0);

    let act_id = ActivationId::generate();
    let turn_id = ragent::domain::ids::TurnId::generate();

    // 1. commit_input
    let (b0, e1) = store
        .commit_input(
            &session_id,
            &act_id,
            &cfg.config_ref,
            vec![Item::user_message("Turn 1")],
            None,
            None,
        )
        .unwrap();
    assert_eq!(b0.batch_seq.as_u64(), 0);
    assert_eq!(e1.event_seq.as_u64(), 1);
    assert_eq!(
        store.rebuild_count(),
        0,
        "commit_input must NOT trigger full replay"
    );

    // 2. append_event (ActivationStarted)
    let e2 = store
        .append_event(
            &session_id,
            SessionEvent::ActivationStarted,
            Some(act_id.clone()),
            None,
        )
        .unwrap();
    assert_eq!(e2.event_seq.as_u64(), 2);
    assert_eq!(
        store.rebuild_count(),
        0,
        "append_event must NOT trigger full replay"
    );

    // 3. commit_model_output
    let (b1, e3) = store
        .commit_model_output(
            &session_id,
            &act_id,
            &turn_id,
            Some("resp_1".into()),
            vec![Item::assistant_message("Assistant step 1")],
            None,
        )
        .unwrap();
    assert_eq!(b1.batch_seq.as_u64(), 1);
    assert_eq!(e3.event_seq.as_u64(), 3);
    assert_eq!(
        store.rebuild_count(),
        0,
        "commit_model_output must NOT trigger full replay"
    );

    // 4. commit_tool_output
    let (b2, e4) = store
        .commit_tool_output(
            &session_id,
            &act_id,
            &turn_id,
            "call_1",
            "shell",
            true,
            Some(10),
            vec![Item::FunctionCallOutput {
                id: None,
                call_id: "call_1".into(),
                output: openresponses_rust::FunctionOutput::Text("output text".into()),
                status: None,
            }],
        )
        .unwrap();
    assert_eq!(b2.batch_seq.as_u64(), 2);
    assert_eq!(e4.event_seq.as_u64(), 4);
    assert_eq!(
        store.rebuild_count(),
        0,
        "commit_tool_output must NOT trigger full replay"
    );

    // 5. append_event (ActivationCompleted)
    let e5 = store
        .append_event(
            &session_id,
            SessionEvent::ActivationCompleted { usage: None },
            Some(act_id.clone()),
            Some(turn_id),
        )
        .unwrap();
    assert_eq!(e5.event_seq.as_u64(), 5);
    assert_eq!(
        store.rebuild_count(),
        0,
        "append_event must NOT trigger full replay"
    );

    // 6. Normal get_status is O(1) and does not trigger rebuild
    let status = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(status.event_count, 6); // 0 (SessionCreated), 1, 2, 3, 4, 5
    assert_eq!(status.batch_count, 3); // 0, 1, 2
    assert_eq!(status.local_item_count, 3);
    assert_eq!(
        store.rebuild_count(),
        0,
        "normal get_status must NOT trigger full replay"
    );

    // 7. Intentionally corrupt projection -> verify rebuild_count increments by 1
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE session_status SET projected_through_event_seq = 0 WHERE session_id = ?1",
            rusqlite::params![session_id.as_str()],
        )
        .unwrap();
    }

    let status_rebuilt = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(status_rebuilt.event_count, 6);
    assert_eq!(
        store.rebuild_count(),
        1,
        "corrupted projection must trigger exactly 1 rebuild"
    );
}
