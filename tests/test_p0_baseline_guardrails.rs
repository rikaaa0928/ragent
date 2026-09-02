mod common;

use common::{file_editor_component_path, image_viewer_component_path, shell_component_path};
use openresponses_rust::Item;
use ragent::control::service::ControlService;
use ragent::hooks::manager::HookManager;
use ragent::hooks::protocol::*;
use ragent::hooks::runtime::WasmPlugin;
use ragent::store::schema::{SCHEMA_MAJOR, SCHEMA_MINOR};
use ragent::store::sqlite::{SqliteControlStore, StoreError};
use ragent::AgentConfig;

#[tokio::test]
async fn test_p0_extensions_load_metadata_and_shutdown() {
    // 1. shell extension
    let shell_plugin = WasmPlugin::load_from_file("shell", &shell_component_path())
        .await
        .expect("shell extension load");
    assert_eq!(shell_plugin.metadata().id, "shell");

    // 2. file_editor extension
    let editor_plugin = WasmPlugin::load_from_file("file_editor", &file_editor_component_path())
        .await
        .expect("file_editor extension load");
    assert_eq!(editor_plugin.metadata().id, "file_editor");

    // 3. image_viewer extension
    let image_plugin = WasmPlugin::load_from_file("image_viewer", &image_viewer_component_path())
        .await
        .expect("image_viewer extension load");
    assert_eq!(image_plugin.metadata().id, "image_viewer");

    // HookManager lifecycle
    let mut manager = HookManager::empty();
    manager.add_plugin(shell_plugin).unwrap();
    manager.add_plugin(editor_plugin).unwrap();
    manager.add_plugin(image_plugin).unwrap();

    manager.initialize().await.expect("initialize plugins");

    let (draft, _) = manager
        .transform_agent_draft(
            HOOK_AGENT_PREPARE,
            None,
            AgentDraft {
                system_prompt: "test system prompt".into(),
                model: ModelDraft::new("test-model"),
                tools: vec![],
                context: None,
            },
        )
        .await
        .expect("transform draft");

    // All 3 extensions register their tools: shell (1), file_editor (2: write, replace), image_viewer (1) = 4 tools
    assert_eq!(draft.tools.len(), 4);

    manager.shutdown().await.expect("shutdown plugins");
}

#[test]
fn test_p0_open_responses_item_round_trip() {
    let user_msg = Item::user_message("Hello from user");
    let serialized = serde_json::to_string(&user_msg).unwrap();
    let deserialized: Item = serde_json::from_str(&serialized).unwrap();
    assert_eq!(user_msg, deserialized);

    let func_call = Item::FunctionCall {
        id: Some("fc_1".into()),
        call_id: "call_abc_123".into(),
        name: "shell".into(),
        arguments: r#"{"command":"echo 1"}"#.into(),
        status: None,
    };
    let serialized_fc = serde_json::to_string(&func_call).unwrap();
    let deserialized_fc: Item = serde_json::from_str(&serialized_fc).unwrap();
    assert_eq!(func_call, deserialized_fc);
}

#[test]
fn test_p0_guardrail_never_writes_old_session_files() {
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_path = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace_path).unwrap();

    let store_dir = temp_dir.path().join(".ragent/store");
    let store_path = store_dir.join("control.sqlite3");

    let config = AgentConfig::new("https://example.invalid", "test_key", "test-model");
    let service = ControlService::open(&store_path, config).expect("open control service");

    let spec = service
        .create_session(&workspace_path, Some("Test prompt"), None)
        .expect("create session");

    // Verify sqlite file exists
    assert!(store_path.exists());

    // Verify that NO old .ragent/sessions/*.json file was written
    let old_sessions_dir = workspace_path.join(".ragent/sessions");
    assert!(!old_sessions_dir.exists());

    let old_sessions_in_temp = temp_dir.path().join(".ragent/sessions");
    assert!(!old_sessions_in_temp.exists());

    // Verify spec has immutable properties
    assert_eq!(spec.format_version, 1);
    assert_eq!(spec.basic_system_prompt, "Test prompt");
}

#[test]
fn test_p0_rejects_unsupported_schema_version() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("control.sqlite3");

    // Create DB with unsupported schema_major
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(ragent::store::schema::INIT_SQL).unwrap();
        conn.execute(
            "INSERT INTO store_meta (key, value) VALUES (?1, ?2)",
            rusqlite::params!["schema_major", (SCHEMA_MAJOR + 1).to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO store_meta (key, value) VALUES (?1, ?2)",
            rusqlite::params!["schema_minor", SCHEMA_MINOR.to_string()],
        )
        .unwrap();
    }

    // Opening store must fail with UnsupportedSchemaVersion
    let err = SqliteControlStore::open(&db_path).unwrap_err();
    match err {
        StoreError::UnsupportedSchemaVersion {
            found_major,
            expected_major,
            ..
        } => {
            assert_eq!(found_major, SCHEMA_MAJOR + 1);
            assert_eq!(expected_major, SCHEMA_MAJOR);
        }
        other => panic!("Expected UnsupportedSchemaVersion, got {:?}", other),
    }
}
