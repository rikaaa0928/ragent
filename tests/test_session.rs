use openresponses_rust::Item;
use ragent::{AgentError, SessionData, SessionStore};

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
