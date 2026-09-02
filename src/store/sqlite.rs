use crate::domain::batch::{ActivationRequestMeta, ItemBatch, ItemBatchKind};
use crate::domain::config::ConfigRevision;
use crate::domain::event::{SessionEvent, SessionEventEnvelope};
use crate::domain::ids::{
    ActivationId, BatchSeq, CommandId, ConfigRef, EventSeq, LocalItemSeq, SessionId, TurnId,
    WorkspaceId,
};
use crate::domain::session::{SessionPhase, SessionSpec, SessionStatus};
use crate::domain::workspace::WorkspaceSpec;
use crate::store::projection::{rebuild_session_status_from_facts, ProjectionError};
use crate::store::schema::*;
use openresponses_rust::{Item, Usage};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "Unsupported schema version: found major={found_major}.{found_minor}, expected major={expected_major}.{expected_minor}"
    )]
    UnsupportedSchemaVersion {
        found_major: u32,
        found_minor: u32,
        expected_major: u32,
        expected_minor: u32,
    },
    #[error("Session not found: {0}")]
    SessionNotFound(SessionId),
    #[error("Session already exists: {0}")]
    SessionAlreadyExists(SessionId),
    #[error("Workspace not found: {0}")]
    WorkspaceNotFound(WorkspaceId),
    #[error("Config revision not found: {0}")]
    ConfigNotFound(ConfigRef),
    #[error("Session is not open: {session_id} (phase: {phase:?})")]
    SessionNotOpen {
        session_id: SessionId,
        phase: SessionPhase,
    },
    #[error("Active activation conflict in session {session_id}: {active_id}")]
    ActiveActivationConflict {
        session_id: SessionId,
        active_id: ActivationId,
    },
    #[error("Projection error: {0}")]
    Projection(#[from] ProjectionError),
    #[error("Integrity check failed: {0}")]
    IntegrityCheckFailed(String),
    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),
}

#[derive(Debug, Clone)]
pub struct SqliteControlStore {
    conn: Arc<Mutex<Connection>>,
    db_path: Option<std::path::PathBuf>,
    rebuild_count: Arc<std::sync::atomic::AtomicU64>,
}

impl SqliteControlStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: Some(path.to_path_buf()),
            rebuild_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        store.init_schema()?;
        store.startup_check_and_recover()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: None,
            rebuild_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        };
        store.init_schema()?;
        store.startup_check_and_recover()?;
        Ok(store)
    }

    pub fn rebuild_count(&self) -> u64 {
        self.rebuild_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    pub fn lock_dir(&self) -> Option<std::path::PathBuf> {
        self.db_path
            .as_ref()
            .map(|p| p.parent().unwrap_or_else(|| Path::new(".")).join("locks"))
    }

    fn init_schema(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(INIT_SQL)?;

        // Check or write store_meta
        let mut stmt = conn.prepare("SELECT value FROM store_meta WHERE key = ?1")?;
        let major_str: Option<String> = stmt
            .query_row(params!["schema_major"], |row| row.get(0))
            .optional()?;

        match major_str {
            Some(maj) => {
                let major: u32 = maj
                    .parse()
                    .map_err(|_| StoreError::ConstraintViolation("Invalid schema_major".into()))?;
                let minor: u32 = stmt
                    .query_row(params!["schema_minor"], |row| row.get(0))
                    .optional()?
                    .and_then(|m: String| m.parse().ok())
                    .unwrap_or(0);

                if major != SCHEMA_MAJOR {
                    return Err(StoreError::UnsupportedSchemaVersion {
                        found_major: major,
                        found_minor: minor,
                        expected_major: SCHEMA_MAJOR,
                        expected_minor: SCHEMA_MINOR,
                    });
                }
            }
            None => {
                conn.execute(
                    "INSERT INTO store_meta (key, value) VALUES (?1, ?2)",
                    params!["store_format", STORE_FORMAT],
                )?;
                conn.execute(
                    "INSERT INTO store_meta (key, value) VALUES (?1, ?2)",
                    params!["schema_major", SCHEMA_MAJOR.to_string()],
                )?;
                conn.execute(
                    "INSERT INTO store_meta (key, value) VALUES (?1, ?2)",
                    params!["schema_minor", SCHEMA_MINOR.to_string()],
                )?;
                conn.execute(
                    "INSERT INTO store_meta (key, value) VALUES (?1, ?2)",
                    params!["open_responses_schema", OPEN_RESPONSES_SCHEMA],
                )?;
            }
        }
        Ok(())
    }

    pub fn startup_check_and_recover(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();

        // 1. Foreign key check
        let mut fk_stmt = conn.prepare("PRAGMA foreign_key_check;")?;
        let mut fk_rows = fk_stmt.query([])?;
        if let Some(row) = fk_rows.next()? {
            let table: String = row.get(0)?;
            let rowid: i64 = row.get(1)?;
            return Err(StoreError::IntegrityCheckFailed(format!(
                "Foreign key check failed in table {} rowid {}",
                table, rowid
            )));
        }
        drop(fk_rows);
        drop(fk_stmt);

        // 2. Quick check
        let mut qc_stmt = conn.prepare("PRAGMA quick_check;")?;
        let qc_res: String = qc_stmt.query_row([], |row| row.get(0))?;
        if qc_res != "ok" {
            return Err(StoreError::IntegrityCheckFailed(format!(
                "PRAGMA quick_check returned: {}",
                qc_res
            )));
        }
        drop(qc_stmt);

        Ok(())
    }

    pub fn recover_interrupted_session(
        &self,
        session_id: &SessionId,
        reason: &str,
    ) -> Result<Option<SessionEventEnvelope>, StoreError> {
        let spec = self
            .get_session(session_id)?
            .ok_or_else(|| StoreError::SessionNotFound(session_id.clone()))?;
        let batches = self.read_batches(session_id)?;
        let mut events = self.read_events(session_id)?;

        // Check if there is an active activation that was never terminated
        let mut active_act = None;
        let mut active_turn = None;
        for env in &events {
            match &env.event {
                SessionEvent::ActivationRequested { .. } | SessionEvent::ActivationStarted => {
                    active_act = env.activation_id.clone();
                }
                SessionEvent::TurnStarted => {
                    active_turn = env.turn_id.clone();
                }
                SessionEvent::ActivationCompleted { .. }
                | SessionEvent::ActivationFailed { .. }
                | SessionEvent::ActivationCancelled
                | SessionEvent::ActivationInterrupted { .. } => {
                    active_act = None;
                    active_turn = None;
                }
                _ => {}
            }
        }

        if let Some(act_id) = active_act {
            // Append interrupted event
            let env = self.append_event(
                session_id,
                SessionEvent::ActivationInterrupted {
                    reason: reason.to_string(),
                },
                Some(act_id),
                active_turn,
            )?;
            events.push(env.clone());

            // Rebuild status and ensure session_status table is up to date
            let status = rebuild_session_status_from_facts(&spec, &batches, &events)?;
            self.save_session_status(
                session_id,
                &status,
                events.len() as u64,
                batches.len() as u64,
            )?;

            Ok(Some(env))
        } else {
            Ok(None)
        }
    }

    fn save_session_status(
        &self,
        session_id: &SessionId,
        status: &SessionStatus,
        event_seq_count: u64,
        batch_seq_count: u64,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let payload = serde_json::to_string(status)?;
        conn.execute(
            "INSERT INTO session_status (session_id, projected_through_event_seq, projected_through_batch_seq, payload_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session_id) DO UPDATE SET
                projected_through_event_seq = excluded.projected_through_event_seq,
                projected_through_batch_seq = excluded.projected_through_batch_seq,
                payload_json = excluded.payload_json",
            params![
                session_id.as_str(),
                event_seq_count,
                batch_seq_count,
                payload
            ],
        )?;
        Ok(())
    }

    pub fn ensure_workspace(&self, ws: &WorkspaceSpec) -> Result<WorkspaceSpec, StoreError> {
        let conn = self.conn.lock().unwrap();
        let payload = serde_json::to_string(ws)?;
        conn.execute(
            "INSERT INTO workspaces (workspace_id, payload_json) VALUES (?1, ?2)
             ON CONFLICT(workspace_id) DO NOTHING",
            params![ws.id.as_str(), payload],
        )?;
        drop(conn);
        Ok(ws.clone())
    }

    pub fn get_workspace(&self, id: &WorkspaceId) -> Result<Option<WorkspaceSpec>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT payload_json FROM workspaces WHERE workspace_id = ?1")?;
        let json_str: Option<String> = stmt
            .query_row(params![id.as_str()], |row| row.get(0))
            .optional()?;
        match json_str {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn ensure_config(&self, cfg: &ConfigRevision) -> Result<ConfigRef, StoreError> {
        if cfg
            .response_template
            .model
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return Err(StoreError::ConstraintViolation(
                "ConfigRevision requires a non-empty model name".to_string(),
            ));
        }

        let computed_ref = cfg.compute_self_ref();
        if cfg.config_ref != computed_ref {
            return Err(StoreError::ConstraintViolation(format!(
                "ConfigRef mismatch: declared '{}' does not match computed '{}'",
                cfg.config_ref, computed_ref
            )));
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT payload_json FROM configs WHERE config_ref = ?1")?;
        let existing: Option<String> = stmt
            .query_row(params![cfg.config_ref.as_str()], |row| row.get(0))
            .optional()?;

        if let Some(existing_json) = existing {
            let existing_cfg: ConfigRevision = serde_json::from_str(&existing_json)?;
            if existing_cfg.response_template != cfg.response_template
                || existing_cfg.extensions != cfg.extensions
                || existing_cfg.context_summary != cfg.context_summary
            {
                return Err(StoreError::ConstraintViolation(format!(
                    "ConfigRef collision: '{}' already exists with conflicting payload",
                    cfg.config_ref
                )));
            }
            return Ok(cfg.config_ref.clone());
        }

        let payload = serde_json::to_string(cfg)?;
        conn.execute(
            "INSERT INTO configs (config_ref, payload_json) VALUES (?1, ?2)",
            params![cfg.config_ref.as_str(), payload],
        )?;
        Ok(cfg.config_ref.clone())
    }

    pub fn get_config(&self, cfg_ref: &ConfigRef) -> Result<Option<ConfigRevision>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT payload_json FROM configs WHERE config_ref = ?1")?;
        let json_str: Option<String> = stmt
            .query_row(params![cfg_ref.as_str()], |row| row.get(0))
            .optional()?;
        match json_str {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn create_session(
        &self,
        spec: &SessionSpec,
        command_id: Option<CommandId>,
        idempotency_key: Option<String>,
    ) -> Result<SessionSpec, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        // Verify workspace exists
        let ws_exists: bool = tx
            .query_row(
                "SELECT 1 FROM workspaces WHERE workspace_id = ?1",
                params![spec.workspace_ref.as_str()],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !ws_exists {
            return Err(StoreError::WorkspaceNotFound(spec.workspace_ref.clone()));
        }

        // Verify config exists
        let cfg_exists: bool = tx
            .query_row(
                "SELECT 1 FROM configs WHERE config_ref = ?1",
                params![spec.default_config_ref.as_str()],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !cfg_exists {
            return Err(StoreError::ConfigNotFound(spec.default_config_ref.clone()));
        }

        // Insert session
        let spec_json = serde_json::to_string(spec)?;
        let created_at_str = spec.created_at.to_rfc3339();
        let inserted = tx.execute(
            "INSERT INTO sessions (session_id, created_at, spec_json) VALUES (?1, ?2, ?3)",
            params![spec.id.as_str(), created_at_str, spec_json],
        );
        if let Err(e) = inserted {
            if let rusqlite::Error::SqliteFailure(ref f, _) = e {
                if f.code == rusqlite::ffi::ErrorCode::ConstraintViolation {
                    return Err(StoreError::SessionAlreadyExists(spec.id.clone()));
                }
            }
            return Err(StoreError::Sqlite(e));
        }

        // Insert initial SessionCreated event (event_seq = 0)
        let event = SessionEvent::SessionCreated {
            command_id,
            idempotency_key,
        };
        let event_envelope =
            SessionEventEnvelope::new(EventSeq::ZERO, spec.id.clone(), None, None, event);
        let event_json = serde_json::to_string(&event_envelope)?;
        tx.execute(
            "INSERT INTO events (session_id, event_seq, kind, activation_id, batch_seq, turn_id, call_id, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                spec.id.as_str(),
                0,
                event_envelope.event.kind_str(),
                Option::<String>::None,
                Option::<u64>::None,
                Option::<String>::None,
                Option::<String>::None,
                event_json
            ],
        )?;

        // Insert initial session_status
        let initial_status = SessionStatus::initial();
        let status_json = serde_json::to_string(&initial_status)?;
        tx.execute(
            "INSERT INTO session_status (session_id, projected_through_event_seq, projected_through_batch_seq, payload_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![spec.id.as_str(), 1, 0, status_json],
        )?;

        tx.commit()?;
        Ok(spec.clone())
    }

    pub fn get_session(&self, id: &SessionId) -> Result<Option<SessionSpec>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT spec_json FROM sessions WHERE session_id = ?1")?;
        let json_str: Option<String> = stmt
            .query_row(params![id.as_str()], |row| row.get(0))
            .optional()?;
        match json_str {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_sessions(&self) -> Result<Vec<(SessionSpec, SessionStatus)>, StoreError> {
        let specs: Vec<SessionSpec> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt =
                conn.prepare("SELECT spec_json FROM sessions ORDER BY created_at DESC")?;
            let rows = stmt.query_map([], |row| {
                let spec_json: String = row.get(0)?;
                Ok(spec_json)
            })?;

            let mut list = Vec::new();
            for r in rows {
                let spec_json = r?;
                list.push(serde_json::from_str(&spec_json)?);
            }
            list
        };

        let mut results = Vec::with_capacity(specs.len());
        for spec in specs {
            let status = self
                .get_or_rebuild_status(&spec.id)?
                .unwrap_or_else(SessionStatus::initial);
            results.push((spec, status));
        }
        Ok(results)
    }

    pub fn get_or_rebuild_status(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionStatus>, StoreError> {
        let conn = self.conn.lock().unwrap();

        // Check session existence
        let spec_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sessions WHERE session_id = ?1",
                params![id.as_str()],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);

        if !spec_exists {
            return Ok(None);
        }

        let proj_info: Option<(u64, u64, String)> = conn
            .query_row(
                "SELECT projected_through_event_seq, projected_through_batch_seq, payload_json
                 FROM session_status WHERE session_id = ?1",
                params![id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        let event_count: u64 = conn.query_row(
            "SELECT COUNT(*) FROM events WHERE session_id = ?1",
            params![id.as_str()],
            |row| row.get(0),
        )?;

        let batch_count: u64 = conn.query_row(
            "SELECT COUNT(*) FROM batches WHERE session_id = ?1",
            params![id.as_str()],
            |row| row.get(0),
        )?;

        if let Some((proj_events, proj_batches, payload_str)) = proj_info {
            if proj_events == event_count && proj_batches == batch_count {
                if let Ok(status) = serde_json::from_str::<SessionStatus>(&payload_str) {
                    return Ok(Some(status));
                }
                // Corrupted/unparseable payload_json: fall through to rebuild below
            }
        }

        drop(conn);
        // Missing or lagging: rebuild atomically
        let rebuilt = self.rebuild_status(id)?;
        Ok(Some(rebuilt))
    }

    pub fn get_status(&self, id: &SessionId) -> Result<Option<SessionStatus>, StoreError> {
        self.get_or_rebuild_status(id)
    }

    pub fn commit_input(
        &self,
        session_id: &SessionId,
        activation_id: &ActivationId,
        config_ref: &ConfigRef,
        items: Vec<Item>,
        command_id: Option<CommandId>,
        idempotency_key: Option<String>,
    ) -> Result<(ItemBatch, SessionEventEnvelope), StoreError> {
        if items.is_empty() {
            return Err(StoreError::ConstraintViolation(
                "Cannot commit empty input items".into(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        // Verify and get session status at start of transaction
        let mut status = tx_get_or_rebuild_status(&tx, session_id, Some(&self.rebuild_count))?;

        if status.phase != SessionPhase::Open {
            return Err(StoreError::SessionNotOpen {
                session_id: session_id.clone(),
                phase: status.phase,
            });
        }

        if let Some(ref act) = status.active_activation_id {
            return Err(StoreError::ActiveActivationConflict {
                session_id: session_id.clone(),
                active_id: act.clone(),
            });
        }

        let batch_seq = BatchSeq::new(status.batch_count);
        let first_local_seq = LocalItemSeq::new(status.local_item_count);
        let event_seq = EventSeq::new(status.event_count);

        let batch = ItemBatch::new(
            session_id.clone(),
            batch_seq,
            first_local_seq,
            ItemBatchKind::Input,
            activation_id.clone(),
            None,
            None,
            Some(ActivationRequestMeta {
                command_id: command_id.clone(),
                idempotency_key: idempotency_key.clone(),
                config_ref: config_ref.clone(),
            }),
            items,
        )
        .map_err(StoreError::ConstraintViolation)?;

        let last_local_item_seq = batch.last_local_item_seq();
        let batch_json = serde_json::to_string(&batch)?;

        tx.execute(
            "INSERT INTO batches (session_id, batch_seq, first_local_item_seq, last_local_item_seq, kind, activation_id, turn_id, response_id, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session_id.as_str(),
                batch_seq.as_u64(),
                first_local_seq.as_u64(),
                last_local_item_seq.as_u64(),
                batch.kind.as_str(),
                activation_id.as_str(),
                Option::<String>::None,
                Option::<String>::None,
                batch_json,
            ],
        )?;

        let event = SessionEvent::ActivationRequested {
            config_ref: config_ref.clone(),
            command_id,
            idempotency_key,
            input_batch_seq: batch_seq,
        };
        let event_envelope = SessionEventEnvelope::new(
            event_seq,
            session_id.clone(),
            Some(activation_id.clone()),
            None,
            event,
        );
        let event_json = serde_json::to_string(&event_envelope)?;

        tx.execute(
            "INSERT INTO events (session_id, event_seq, kind, activation_id, batch_seq, turn_id, call_id, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session_id.as_str(),
                event_seq.as_u64(),
                event_envelope.event.kind_str(),
                activation_id.as_str(),
                batch_seq.as_u64(),
                Option::<String>::None,
                Option::<String>::None,
                event_json,
            ],
        )?;

        // Update status incrementally
        let local_items_total = last_local_item_seq.as_u64() + 1;
        status.active_activation_id = Some(activation_id.clone());
        status.local_item_count = local_items_total;
        status.effective_context_item_count = local_items_total;
        status.batch_count += 1;
        status.event_count += 1;
        status.updated_at = event_envelope.created_at;

        let status_json = serde_json::to_string(&status)?;
        tx.execute(
            "UPDATE session_status SET
                projected_through_event_seq = ?2,
                projected_through_batch_seq = ?3,
                payload_json = ?4
             WHERE session_id = ?1",
            params![
                session_id.as_str(),
                status.event_count,
                status.batch_count,
                status_json
            ],
        )?;

        tx.commit()?;
        Ok((batch, event_envelope))
    }

    pub fn commit_model_output(
        &self,
        session_id: &SessionId,
        activation_id: &ActivationId,
        turn_id: &TurnId,
        response_id: Option<String>,
        items: Vec<Item>,
        usage: Option<Usage>,
    ) -> Result<(ItemBatch, SessionEventEnvelope), StoreError> {
        if items.is_empty() {
            return Err(StoreError::ConstraintViolation(
                "Cannot commit empty model output items".into(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let mut status = tx_get_or_rebuild_status(&tx, session_id, Some(&self.rebuild_count))?;

        let batch_seq = BatchSeq::new(status.batch_count);
        let first_local_seq = LocalItemSeq::new(status.local_item_count);
        let event_seq = EventSeq::new(status.event_count);

        let batch = ItemBatch::new(
            session_id.clone(),
            batch_seq,
            first_local_seq,
            ItemBatchKind::ModelOutput,
            activation_id.clone(),
            Some(turn_id.clone()),
            response_id.clone(),
            None,
            items,
        )
        .map_err(StoreError::ConstraintViolation)?;

        let last_local_item_seq = batch.last_local_item_seq();
        let batch_json = serde_json::to_string(&batch)?;

        tx.execute(
            "INSERT INTO batches (session_id, batch_seq, first_local_item_seq, last_local_item_seq, kind, activation_id, turn_id, response_id, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session_id.as_str(),
                batch_seq.as_u64(),
                first_local_seq.as_u64(),
                last_local_item_seq.as_u64(),
                batch.kind.as_str(),
                activation_id.as_str(),
                turn_id.as_str(),
                response_id.as_deref(),
                batch_json,
            ],
        )?;

        let event = SessionEvent::TurnCompleted {
            response_id,
            output_batch_seq: Some(batch_seq),
            usage,
        };
        let event_envelope = SessionEventEnvelope::new(
            event_seq,
            session_id.clone(),
            Some(activation_id.clone()),
            Some(turn_id.clone()),
            event,
        );
        let event_json = serde_json::to_string(&event_envelope)?;

        tx.execute(
            "INSERT INTO events (session_id, event_seq, kind, activation_id, batch_seq, turn_id, call_id, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session_id.as_str(),
                event_seq.as_u64(),
                event_envelope.event.kind_str(),
                activation_id.as_str(),
                batch_seq.as_u64(),
                turn_id.as_str(),
                Option::<String>::None,
                event_json,
            ],
        )?;

        let local_items_total = last_local_item_seq.as_u64() + 1;
        status.local_item_count = local_items_total;
        status.effective_context_item_count = local_items_total;
        status.batch_count += 1;
        status.event_count += 1;
        status.updated_at = event_envelope.created_at;

        let status_json = serde_json::to_string(&status)?;
        tx.execute(
            "UPDATE session_status SET
                projected_through_event_seq = ?2,
                projected_through_batch_seq = ?3,
                payload_json = ?4
             WHERE session_id = ?1",
            params![
                session_id.as_str(),
                status.event_count,
                status.batch_count,
                status_json
            ],
        )?;

        tx.commit()?;
        Ok((batch, event_envelope))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_tool_output(
        &self,
        session_id: &SessionId,
        activation_id: &ActivationId,
        turn_id: &TurnId,
        call_id: &str,
        tool_name: &str,
        success: bool,
        duration_ms: Option<u64>,
        items: Vec<Item>,
    ) -> Result<(ItemBatch, SessionEventEnvelope), StoreError> {
        if items.is_empty() {
            return Err(StoreError::ConstraintViolation(
                "Cannot commit empty tool output items".into(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let mut status = tx_get_or_rebuild_status(&tx, session_id, Some(&self.rebuild_count))?;

        let batch_seq = BatchSeq::new(status.batch_count);
        let first_local_seq = LocalItemSeq::new(status.local_item_count);
        let event_seq = EventSeq::new(status.event_count);

        let batch = ItemBatch::new(
            session_id.clone(),
            batch_seq,
            first_local_seq,
            ItemBatchKind::ToolOutput,
            activation_id.clone(),
            Some(turn_id.clone()),
            None,
            None,
            items,
        )
        .map_err(StoreError::ConstraintViolation)?;

        let last_local_item_seq = batch.last_local_item_seq();
        let batch_json = serde_json::to_string(&batch)?;

        tx.execute(
            "INSERT INTO batches (session_id, batch_seq, first_local_item_seq, last_local_item_seq, kind, activation_id, turn_id, response_id, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                session_id.as_str(),
                batch_seq.as_u64(),
                first_local_seq.as_u64(),
                last_local_item_seq.as_u64(),
                batch.kind.as_str(),
                activation_id.as_str(),
                turn_id.as_str(),
                Option::<String>::None,
                batch_json,
            ],
        )?;

        let event = SessionEvent::ToolCallFinished {
            call_id: call_id.to_string(),
            name: tool_name.to_string(),
            success,
            duration_ms,
            output_batch_seq: batch_seq,
        };
        let event_envelope = SessionEventEnvelope::new(
            event_seq,
            session_id.clone(),
            Some(activation_id.clone()),
            Some(turn_id.clone()),
            event,
        );
        let event_json = serde_json::to_string(&event_envelope)?;

        tx.execute(
            "INSERT INTO events (session_id, event_seq, kind, activation_id, batch_seq, turn_id, call_id, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session_id.as_str(),
                event_seq.as_u64(),
                event_envelope.event.kind_str(),
                activation_id.as_str(),
                batch_seq.as_u64(),
                turn_id.as_str(),
                call_id,
                event_json,
            ],
        )?;

        let local_items_total = last_local_item_seq.as_u64() + 1;
        status.local_item_count = local_items_total;
        status.effective_context_item_count = local_items_total;
        status.batch_count += 1;
        status.event_count += 1;
        status.updated_at = event_envelope.created_at;

        let status_json = serde_json::to_string(&status)?;
        tx.execute(
            "UPDATE session_status SET
                projected_through_event_seq = ?2,
                projected_through_batch_seq = ?3,
                payload_json = ?4
             WHERE session_id = ?1",
            params![
                session_id.as_str(),
                status.event_count,
                status.batch_count,
                status_json
            ],
        )?;

        tx.commit()?;
        Ok((batch, event_envelope))
    }

    pub fn append_event(
        &self,
        session_id: &SessionId,
        event: SessionEvent,
        activation_id: Option<ActivationId>,
        turn_id: Option<TurnId>,
    ) -> Result<SessionEventEnvelope, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let mut status = tx_get_or_rebuild_status(&tx, session_id, Some(&self.rebuild_count))?;

        let event_seq = EventSeq::new(status.event_count);
        let event_envelope = SessionEventEnvelope::new(
            event_seq,
            session_id.clone(),
            activation_id.clone(),
            turn_id.clone(),
            event,
        );
        let event_json = serde_json::to_string(&event_envelope)?;

        tx.execute(
            "INSERT INTO events (session_id, event_seq, kind, activation_id, batch_seq, turn_id, call_id, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session_id.as_str(),
                event_seq.as_u64(),
                event_envelope.event.kind_str(),
                activation_id.as_ref().map(|a| a.as_str()),
                Option::<u64>::None,
                turn_id.as_ref().map(|t| t.as_str()),
                Option::<String>::None,
                event_json,
            ],
        )?;

        match &event_envelope.event {
            SessionEvent::ActivationStarted | SessionEvent::ActivationRequested { .. } => {
                status.active_activation_id = activation_id;
            }
            SessionEvent::ActivationCompleted { .. } => {
                status.active_activation_id = None;
            }
            SessionEvent::ActivationFailed { error } => {
                status.active_activation_id = None;
                status.last_error = Some(error.clone());
            }
            SessionEvent::ActivationCancelled => {
                status.active_activation_id = None;
            }
            SessionEvent::ActivationInterrupted { reason } => {
                status.active_activation_id = None;
                status.last_error = Some(reason.clone());
            }
            SessionEvent::SessionClosed => {
                status.phase = SessionPhase::Closed;
            }
            SessionEvent::SessionArchived => {
                status.phase = SessionPhase::Archived;
            }
            _ => {}
        }

        status.event_count += 1;
        status.updated_at = event_envelope.created_at;

        let status_json = serde_json::to_string(&status)?;
        tx.execute(
            "UPDATE session_status SET
                projected_through_event_seq = ?2,
                projected_through_batch_seq = ?3,
                payload_json = ?4
             WHERE session_id = ?1",
            params![
                session_id.as_str(),
                status.event_count,
                status.batch_count,
                status_json
            ],
        )?;

        tx.commit()?;
        Ok(event_envelope)
    }

    pub fn read_local_items(&self, session_id: &SessionId) -> Result<Vec<Item>, StoreError> {
        let batches = self.read_batches(session_id)?;
        let mut items = Vec::new();
        for b in batches {
            items.extend(b.items);
        }
        Ok(items)
    }

    pub fn read_batches(&self, session_id: &SessionId) -> Result<Vec<ItemBatch>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT payload_json FROM batches WHERE session_id = ?1 ORDER BY batch_seq ASC",
        )?;
        let rows = stmt.query_map(params![session_id.as_str()], |row| {
            let json_str: String = row.get(0)?;
            Ok(json_str)
        })?;

        let mut batches = Vec::new();
        for r in rows {
            let json_str = r?;
            batches.push(serde_json::from_str(&json_str)?);
        }
        Ok(batches)
    }

    pub fn read_events(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<SessionEventEnvelope>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT payload_json FROM events WHERE session_id = ?1 ORDER BY event_seq ASC",
        )?;
        let rows = stmt.query_map(params![session_id.as_str()], |row| {
            let json_str: String = row.get(0)?;
            Ok(json_str)
        })?;

        let mut events = Vec::new();
        for r in rows {
            let json_str = r?;
            events.push(serde_json::from_str(&json_str)?);
        }
        Ok(events)
    }

    pub fn rebuild_status(&self, session_id: &SessionId) -> Result<SessionStatus, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

        let spec_json: Option<String> = tx
            .query_row(
                "SELECT spec_json FROM sessions WHERE session_id = ?1",
                params![session_id.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        let spec: SessionSpec = match spec_json {
            Some(s) => serde_json::from_str(&s)?,
            None => return Err(StoreError::SessionNotFound(session_id.clone())),
        };

        let mut batches = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT payload_json FROM batches WHERE session_id = ?1 ORDER BY batch_seq ASC",
            )?;
            let rows = stmt.query_map(params![session_id.as_str()], |row| {
                let json_str: String = row.get(0)?;
                Ok(json_str)
            })?;
            for r in rows {
                batches.push(serde_json::from_str(&r?)?);
            }
        }

        let mut events = Vec::new();
        {
            let mut stmt = tx.prepare(
                "SELECT payload_json FROM events WHERE session_id = ?1 ORDER BY event_seq ASC",
            )?;
            let rows = stmt.query_map(params![session_id.as_str()], |row| {
                let json_str: String = row.get(0)?;
                Ok(json_str)
            })?;
            for r in rows {
                events.push(serde_json::from_str(&r?)?);
            }
        }

        let status = rebuild_session_status_from_facts(&spec, &batches, &events)?;
        self.rebuild_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let payload = serde_json::to_string(&status)?;
        tx.execute(
            "INSERT INTO session_status (session_id, projected_through_event_seq, projected_through_batch_seq, payload_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session_id) DO UPDATE SET
                projected_through_event_seq = excluded.projected_through_event_seq,
                projected_through_batch_seq = excluded.projected_through_batch_seq,
                payload_json = excluded.payload_json",
            params![
                session_id.as_str(),
                events.len() as u64,
                batches.len() as u64,
                payload
            ],
        )?;

        tx.commit()?;
        Ok(status)
    }
}

fn tx_get_or_rebuild_status(
    tx: &rusqlite::Transaction,
    session_id: &SessionId,
    rebuild_counter: Option<&std::sync::atomic::AtomicU64>,
) -> Result<SessionStatus, StoreError> {
    let proj_info: Option<(u64, u64, String)> = tx
        .query_row(
            "SELECT projected_through_event_seq, projected_through_batch_seq, payload_json
             FROM session_status WHERE session_id = ?1",
            params![session_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let event_count: u64 = tx.query_row(
        "SELECT COUNT(*) FROM events WHERE session_id = ?1",
        params![session_id.as_str()],
        |row| row.get(0),
    )?;

    let batch_count: u64 = tx.query_row(
        "SELECT COUNT(*) FROM batches WHERE session_id = ?1",
        params![session_id.as_str()],
        |row| row.get(0),
    )?;

    if let Some((proj_events, proj_batches, payload_str)) = proj_info {
        if proj_events == event_count && proj_batches == batch_count {
            if let Ok(status) = serde_json::from_str::<SessionStatus>(&payload_str) {
                return Ok(status);
            }
            // Corrupted/unparseable payload_json: fall through to rebuild below
        }
    }

    let spec_json: Option<String> = tx
        .query_row(
            "SELECT spec_json FROM sessions WHERE session_id = ?1",
            params![session_id.as_str()],
            |row| row.get(0),
        )
        .optional()?;

    let spec: SessionSpec = match spec_json {
        Some(s) => serde_json::from_str(&s)?,
        None => return Err(StoreError::SessionNotFound(session_id.clone())),
    };

    let mut batches = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT payload_json FROM batches WHERE session_id = ?1 ORDER BY batch_seq ASC",
        )?;
        let rows = stmt.query_map(params![session_id.as_str()], |row| {
            let json_str: String = row.get(0)?;
            Ok(json_str)
        })?;
        for r in rows {
            batches.push(serde_json::from_str(&r?)?);
        }
    }

    let mut events = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT payload_json FROM events WHERE session_id = ?1 ORDER BY event_seq ASC",
        )?;
        let rows = stmt.query_map(params![session_id.as_str()], |row| {
            let json_str: String = row.get(0)?;
            Ok(json_str)
        })?;
        for r in rows {
            events.push(serde_json::from_str(&r?)?);
        }
    }

    let status = rebuild_session_status_from_facts(&spec, &batches, &events)?;
    if let Some(counter) = rebuild_counter {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    let payload = serde_json::to_string(&status)?;
    tx.execute(
        "INSERT INTO session_status (session_id, projected_through_event_seq, projected_through_batch_seq, payload_json)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(session_id) DO UPDATE SET
            projected_through_event_seq = excluded.projected_through_event_seq,
            projected_through_batch_seq = excluded.projected_through_batch_seq,
            payload_json = excluded.payload_json",
        params![
            session_id.as_str(),
            events.len() as u64,
            batches.len() as u64,
            payload
        ],
    )?;

    Ok(status)
}
