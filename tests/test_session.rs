use openresponses_rust::Item;
use ragent::{
    build_basic_system_prompt, ensure_session_tmp_dir, session_tmp_dir, validate_session_id,
    AgentBuilder, AgentConfig, AgentError, ExtensionManager, SessionData, SessionStore,
    DEFAULT_SYSTEM_PROMPT, SESSION_SCHEMA_VERSION,
};
use std::path::PathBuf;

#[test]
fn session_store_crud_and_rejects_path_traversal() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(temp.path().join("sessions"));
    let mut session = SessionData::new("sess_test", "test-model").unwrap();
    session.update_from_context(vec![Item::user_message("What is Rust?")]);
    store.save(&session).unwrap();

    let loaded = store.load("sess_test").unwrap().unwrap();
    assert_eq!(loaded.schema_version, SESSION_SCHEMA_VERSION);
    assert_eq!(loaded.meta.item_count, 1);
    assert_eq!(store.list().unwrap().len(), 1);
    assert!(store.delete("sess_test").unwrap());

    assert!(matches!(
        store.load("../escape"),
        Err(AgentError::InvalidSessionId(_))
    ));
    assert!(matches!(
        SessionData::new("../escape", "model"),
        Err(AgentError::InvalidSessionId(_))
    ));
    assert!(!temp.path().join("escape.json").exists());
}

#[tokio::test]
async fn session_build_creates_temp_dir_and_prepares_stored_basic_prompt() {
    let session_id = format!("sess_test_temp_{}", std::process::id());
    let expected_dir = session_tmp_dir(&session_id);
    let _ = std::fs::remove_dir_all(&expected_dir);

    let session = SessionData::new(&session_id, "test-model").unwrap();
    let expected_basic = session.basic_system_prompt().to_string();
    let config = AgentConfig::new("https://example.com", "fake_key", "test-model");
    let (agent, _) =
        AgentBuilder::from_session_with_manager(session, config, Some(ExtensionManager::empty()))
            .await
            .unwrap();

    assert!(expected_dir.is_dir());
    assert_eq!(agent.session_tmp_dir(), Some(expected_dir.clone()));
    assert_eq!(agent.session_name(), Some(session_id.as_str()));
    assert_eq!(agent.basic_system_prompt(), expected_basic);
    assert_eq!(agent.prepared_system_prompt(), expected_basic);
    assert!(agent.basic_system_prompt().contains("session_tmp"));
    assert!(agent.basic_system_prompt().contains("workspace"));
    assert!(agent
        .basic_system_prompt()
        .contains(&expected_dir.display().to_string()));

    let _ = std::fs::remove_dir_all(&expected_dir);
}

#[tokio::test]
async fn resume_uses_stored_basic_prompt_without_internal_reprocessing() {
    let session_id = format!("sess_frozen_prompt_{}", std::process::id());
    let session = SessionData::new(&session_id, "test-model").unwrap();
    let mut value = serde_json::to_value(session).unwrap();
    value["basic_system_prompt"] = serde_json::json!("frozen basic prompt");
    let frozen: SessionData = serde_json::from_value(value).unwrap();

    let config = AgentConfig::new("https://example.com", "fake_key", "test-model");
    let (agent, _) =
        AgentBuilder::from_session_with_manager(frozen, config, Some(ExtensionManager::empty()))
            .await
            .unwrap();

    assert_eq!(agent.basic_system_prompt(), "frozen basic prompt");
    assert_eq!(agent.prepared_system_prompt(), "frozen basic prompt");
    assert!(!agent.basic_system_prompt().contains("session_tmp"));

    let _ = std::fs::remove_dir_all(session_tmp_dir(&session_id));
}

#[test]
fn session_persists_only_immutable_basic_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(temp.path());
    let mut session = SessionData::new("sess_persist", "test-model").unwrap();
    let original = session.basic_system_prompt().to_string();

    session.update_from_context(vec![Item::user_message("hello")]);
    store.save(&session).unwrap();

    let raw = std::fs::read_to_string(store.session_file_path("sess_persist").unwrap()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["schema_version"], SESSION_SCHEMA_VERSION);
    assert_eq!(json["basic_system_prompt"], original);
    assert!(json.get("system_prompt").is_none());
    assert!(json.get("prepared_system_prompt").is_none());

    let loaded = store.load("sess_persist").unwrap().unwrap();
    assert_eq!(loaded.basic_system_prompt(), original);
}

#[test]
fn session_store_rejects_unsupported_schema_version() {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(temp.path());
    let session = SessionData::new("sess_version", "test-model").unwrap();
    let mut value = serde_json::to_value(session).unwrap();
    value["schema_version"] = serde_json::json!(SESSION_SCHEMA_VERSION + 1);
    std::fs::write(
        store.session_file_path("sess_version").unwrap(),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();

    assert!(matches!(
        store.load("sess_version"),
        Err(AgentError::UnsupportedSessionVersion {
            found: Some(found),
            expected: SESSION_SCHEMA_VERSION,
        }) if found == u64::from(SESSION_SCHEMA_VERSION + 1)
    ));
}

#[test]
fn session_temp_dir_helpers_and_validation() {
    assert_eq!(
        session_tmp_dir("sess_abc_123"),
        PathBuf::from("/tmp/ragent/sess_abc_123")
    );
    assert!(validate_session_id("sess_123_abc-DEF").is_ok());
    assert!(validate_session_id("../escape").is_err());
    assert!(validate_session_id("").is_err());
    assert!(matches!(
        ensure_session_tmp_dir("../escape"),
        Err(AgentError::InvalidSessionId(_))
    ));

    let session = SessionData::new("sess_helper_check", "test-model").unwrap();
    assert_eq!(session.temp_dir(), session_tmp_dir("sess_helper_check"));
    let created = session.ensure_temp_dir().unwrap();
    assert!(created.is_dir());
    let _ = std::fs::remove_dir_all(created);
}

#[test]
fn basic_system_prompt_is_complete_when_session_is_created() {
    let prompt = build_basic_system_prompt("sess_123").unwrap();
    assert!(prompt.starts_with(DEFAULT_SYSTEM_PROMPT));
    assert!(prompt.contains("/tmp/ragent/sess_123"));
    assert_eq!(prompt.matches("## 会话临时目录 (session_tmp)").count(), 1);
}
