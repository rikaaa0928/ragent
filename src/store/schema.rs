pub const STORE_FORMAT: &str = "ragent-sqlite-store";
pub const SCHEMA_MAJOR: u32 = 1;
pub const SCHEMA_MINOR: u32 = 0;
pub const OPEN_RESPONSES_SCHEMA: &str = "openresponses-rust-2026.7.26";

pub const INIT_SQL: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS store_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS configs (
    config_ref TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    spec_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS session_sources (
    child_session_id TEXT NOT NULL,
    source_order INTEGER NOT NULL,
    source_session_id TEXT NOT NULL,
    from_context_pos INTEGER NOT NULL,
    through_context_pos INTEGER NOT NULL,
    PRIMARY KEY (child_session_id, source_order),
    FOREIGN KEY (child_session_id) REFERENCES sessions(session_id),
    FOREIGN KEY (source_session_id) REFERENCES sessions(session_id)
);

CREATE INDEX IF NOT EXISTS idx_session_sources_source ON session_sources(source_session_id);

CREATE TABLE IF NOT EXISTS batches (
    session_id TEXT NOT NULL,
    batch_seq INTEGER NOT NULL,
    first_local_item_seq INTEGER NOT NULL,
    last_local_item_seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    activation_id TEXT NOT NULL,
    turn_id TEXT,
    response_id TEXT,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (session_id, batch_seq),
    FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_batches_local_seq_range
    ON batches(session_id, first_local_item_seq, last_local_item_seq);

CREATE TABLE IF NOT EXISTS events (
    session_id TEXT NOT NULL,
    event_seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    activation_id TEXT,
    batch_seq INTEGER,
    turn_id TEXT,
    call_id TEXT,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (session_id, event_seq),
    FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);

CREATE TABLE IF NOT EXISTS session_status (
    session_id TEXT PRIMARY KEY,
    projected_through_event_seq INTEGER NOT NULL,
    projected_through_batch_seq INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(session_id)
);
"#;
