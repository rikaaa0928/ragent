use openresponses_rust::{CreateResponseBody, Item, Usage};
use ragent::domain::batch::ItemBatchKind;
use ragent::domain::config::ConfigRevision;
use ragent::domain::event::SessionEvent;
use ragent::domain::ids::{ActivationId, BatchSeq, EventSeq, LocalItemSeq, SessionId, TurnId};
use ragent::domain::session::{SessionPhase, SessionSpec};
use ragent::domain::workspace::WorkspaceSpec;
use ragent::store::projection::rebuild_session_status_from_facts;
use ragent::store::sqlite::{SqliteControlStore, StoreError};

#[test]
fn test_p1_workspace_and_content_addressed_config() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteControlStore::open(temp.path().join("control.sqlite3")).unwrap();

    // 1. Ensure workspace
    let ws_spec = WorkspaceSpec::new(temp.path()).unwrap();
    let ws_id = ws_spec.id.clone();
    store.ensure_workspace(&ws_spec).unwrap();

    let fetched_ws = store
        .get_workspace(&ws_id)
        .unwrap()
        .expect("workspace should exist");
    assert_eq!(fetched_ws.id, ws_id);
    assert_eq!(fetched_ws.root, temp.path().canonicalize().unwrap());

    // 2. Ensure content-addressed config
    let template = CreateResponseBody {
        model: Some("test-model".into()),
        temperature: Some(0.5),
        stream: Some(false),
        ..Default::default()
    };
    let cfg_rev = ConfigRevision::new(
        template.clone(),
        vec![],
        ragent::config::ContextSummaryMode::default(),
    );
    let cfg_ref = cfg_rev.config_ref.clone();
    assert!(cfg_ref.as_str().starts_with("sha256:"));

    store.ensure_config(&cfg_rev).unwrap();
    let fetched_cfg = store
        .get_config(&cfg_ref)
        .unwrap()
        .expect("config should exist");
    assert_eq!(fetched_cfg.config_ref, cfg_ref);
    assert_eq!(
        fetched_cfg.response_template.model.as_deref(),
        Some("test-model")
    );

    // Idempotent write returns same ref
    let ref2 = store.ensure_config(&cfg_rev).unwrap();
    assert_eq!(ref2, cfg_ref);
}

#[test]
fn test_p1_session_spec_immutable_and_initial_status() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteControlStore::open(temp.path().join("control.sqlite3")).unwrap();

    let ws = WorkspaceSpec::new(temp.path()).unwrap();
    store.ensure_workspace(&ws).unwrap();

    let cfg = ConfigRevision::new(
        CreateResponseBody {
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
        "Initial basic prompt",
        cfg.config_ref.clone(),
        ws.id.clone(),
    );

    let created = store.create_session(&spec, None, None).unwrap();
    assert_eq!(created.id, session_id);
    assert_eq!(created.basic_system_prompt, "Initial basic prompt");
    assert_eq!(created.sources.len(), 0); // Prototype source must be empty

    // Duplicate session creation fails
    let dup_err = store.create_session(&spec, None, None).unwrap_err();
    assert!(matches!(dup_err, StoreError::SessionAlreadyExists(_)));

    // Status is initialized to Open with 0 batches, 0 items, 1 event (SessionCreated)
    let status = store
        .get_status(&session_id)
        .unwrap()
        .expect("status should exist");
    assert_eq!(status.phase, SessionPhase::Open);
    assert_eq!(status.local_item_count, 0);
    assert_eq!(status.batch_count, 0);
    assert_eq!(status.event_count, 1);
    assert!(status.active_activation_id.is_none());

    // List sessions contains the new session
    let list = store.list_sessions().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].0.id, session_id);
    assert_eq!(list[0].1.phase, SessionPhase::Open);
}

#[test]
fn test_p1_batch_and_event_atomic_commits_and_sequences() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteControlStore::open(temp.path().join("control.sqlite3")).unwrap();

    let ws = WorkspaceSpec::new(temp.path()).unwrap();
    store.ensure_workspace(&ws).unwrap();

    let cfg = ConfigRevision::new(
        CreateResponseBody {
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

    // 1. Commit Input Batch (2 items)
    let input_items = vec![
        Item::user_message("First user message"),
        Item::user_message("Second user message"),
    ];
    let (input_batch, input_env) = store
        .commit_input(
            &session_id,
            &act_id,
            &cfg.config_ref,
            input_items.clone(),
            None,
            None,
        )
        .unwrap();

    assert_eq!(input_batch.batch_seq, BatchSeq::ZERO);
    assert_eq!(input_batch.first_local_item_seq, LocalItemSeq::ZERO);
    assert_eq!(input_batch.last_local_item_seq(), LocalItemSeq::new(1));
    assert_eq!(input_batch.items.len(), 2);
    assert_eq!(input_batch.kind, ItemBatchKind::Input);

    assert_eq!(input_env.event_seq, EventSeq::new(1)); // seq 0 was SessionCreated
    assert!(matches!(
        input_env.event,
        SessionEvent::ActivationRequested { .. }
    ));

    // Status updated: active_activation_id is Some(act_id), local_item_count=2, batch_count=1, event_count=2
    let status1 = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(status1.active_activation_id.as_ref(), Some(&act_id));
    assert_eq!(status1.local_item_count, 2);
    assert_eq!(status1.batch_count, 1);
    assert_eq!(status1.event_count, 2);

    // 2. Commit ModelOutput Batch (1 reasoning item + 1 function call item)
    let turn_id = TurnId::generate();
    let model_items = vec![
        Item::Reasoning {
            id: Some("rs_1".into()),
            status: None,
            content: None,
            summary: vec![],
            encrypted_content: Some("enc_1".into()),
        },
        Item::FunctionCall {
            id: Some("fc_1".into()),
            call_id: "call_1".into(),
            name: "shell".into(),
            arguments: r#"{"command":"ls"}"#.into(),
            status: None,
        },
    ];
    let usage = Usage {
        input_tokens: 50,
        output_tokens: 20,
        total_tokens: 70,
        input_tokens_details: openresponses_rust::InputTokensDetails { cached_tokens: 0 },
        output_tokens_details: openresponses_rust::OutputTokensDetails {
            reasoning_tokens: 10,
        },
    };
    let (model_batch, model_env) = store
        .commit_model_output(
            &session_id,
            &act_id,
            &turn_id,
            Some("resp_123".into()),
            model_items,
            Some(usage),
        )
        .unwrap();

    assert_eq!(model_batch.batch_seq, BatchSeq::new(1));
    assert_eq!(model_batch.first_local_item_seq, LocalItemSeq::new(2));
    assert_eq!(model_batch.last_local_item_seq(), LocalItemSeq::new(3));
    assert_eq!(model_batch.items.len(), 2);
    assert_eq!(model_batch.kind, ItemBatchKind::ModelOutput);

    assert_eq!(model_env.event_seq, EventSeq::new(2));
    assert!(matches!(
        model_env.event,
        SessionEvent::TurnCompleted { .. }
    ));

    // 3. Commit ToolOutput Batch (1 FunctionCallOutput item)
    let tool_items = vec![Item::FunctionCallOutput {
        id: None,
        call_id: "call_1".into(),
        output: openresponses_rust::FunctionOutput::Text("file1.txt\nfile2.txt".into()),
        status: None,
    }];
    let (tool_batch, tool_env) = store
        .commit_tool_output(
            &session_id,
            &act_id,
            &turn_id,
            "call_1",
            "shell",
            true,
            Some(15),
            tool_items,
        )
        .unwrap();

    assert_eq!(tool_batch.batch_seq, BatchSeq::new(2));
    assert_eq!(tool_batch.first_local_item_seq, LocalItemSeq::new(4));
    assert_eq!(tool_batch.last_local_item_seq(), LocalItemSeq::new(4));
    assert_eq!(tool_batch.items.len(), 1);
    assert_eq!(tool_batch.kind, ItemBatchKind::ToolOutput);

    assert_eq!(tool_env.event_seq, EventSeq::new(3));
    assert!(matches!(
        tool_env.event,
        SessionEvent::ToolCallFinished { .. }
    ));

    // 4. Append ActivationCompleted event
    let fin_env = store
        .append_event(
            &session_id,
            SessionEvent::ActivationCompleted { usage: None },
            Some(act_id.clone()),
            Some(turn_id),
        )
        .unwrap();

    assert_eq!(fin_env.event_seq, EventSeq::new(4));

    // Status reflects final counts and cleared active_activation_id
    let final_status = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(final_status.phase, SessionPhase::Open);
    assert_eq!(final_status.local_item_count, 5); // 2 input + 2 model + 1 tool = 5 items
    assert_eq!(final_status.batch_count, 3);
    assert_eq!(final_status.event_count, 5);
    assert!(final_status.active_activation_id.is_none());

    // 5. Read local items returns exact sequential 5 items
    let local_items = store.read_local_items(&session_id).unwrap();
    assert_eq!(local_items.len(), 5);

    // 6. Read batches and events returns all in sequence
    let batches = store.read_batches(&session_id).unwrap();
    assert_eq!(batches.len(), 3);
    let events = store.read_events(&session_id).unwrap();
    assert_eq!(events.len(), 5);
}

#[test]
fn test_p1_rebuild_status_from_empty_status_table() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("control.sqlite3");
    let store = SqliteControlStore::open(&db_path).unwrap();

    let ws = WorkspaceSpec::new(temp.path()).unwrap();
    store.ensure_workspace(&ws).unwrap();

    let cfg = ConfigRevision::new(
        CreateResponseBody {
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

    // Intentionally wipe session_status table
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("DELETE FROM session_status", []).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_status", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    // Querying status automatically reconstructs from facts and restores the projection table
    let rebuilt = store.get_status(&session_id).unwrap().unwrap();
    assert_eq!(rebuilt.phase, status_before.phase);
    assert_eq!(
        rebuilt.active_activation_id,
        status_before.active_activation_id
    );
    assert_eq!(rebuilt.local_item_count, status_before.local_item_count);
    assert_eq!(rebuilt.batch_count, status_before.batch_count);
    assert_eq!(rebuilt.event_count, status_before.event_count);

    // Verify projection table row is now restored
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM session_status", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    // Verify projection reconstruction helper directly
    let batches = store.read_batches(&session_id).unwrap();
    let events = store.read_events(&session_id).unwrap();
    let projected = rebuild_session_status_from_facts(&spec, &batches, &events).unwrap();
    assert_eq!(projected.local_item_count, 1);
    assert_eq!(projected.batch_count, 1);
    assert_eq!(projected.event_count, 2);
}
