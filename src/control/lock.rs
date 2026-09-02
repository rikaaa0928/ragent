use crate::domain::ids::SessionId;
use crate::error::AgentError;
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static IN_MEMORY_ACTIVE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn get_in_memory_active() -> &'static Mutex<Option<String>> {
    IN_MEMORY_ACTIVE.get_or_init(|| Mutex::new(None))
}

#[derive(Debug)]
pub struct RunnerLock {
    _file: Option<File>,
    is_in_memory: bool,
    session_id: SessionId,
}

pub type SessionLock = RunnerLock;

impl RunnerLock {
    pub fn acquire(lock_dir: Option<&Path>, session_id: &SessionId) -> Result<Self, AgentError> {
        if let Some(dir) = lock_dir {
            std::fs::create_dir_all(dir).map_err(|e| {
                AgentError::ToolError(format!("Failed to create lock dir {:?}: {}", dir, e))
            })?;
            // Prototype constraint: Store-level global runner lock
            let lock_path = dir.join("runner.lock");
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock_path)
                .map_err(|e| {
                    AgentError::ToolError(format!(
                        "Failed to open lock file {:?}: {}",
                        lock_path, e
                    ))
                })?;

            match file.try_lock_exclusive() {
                Ok(_) => {
                    let _ = file.set_len(0);
                    let _ = writeln!(
                        file,
                        "session_id={}\npid={}",
                        session_id,
                        std::process::id()
                    );
                    Ok(Self {
                        _file: Some(file),
                        is_in_memory: false,
                        session_id: session_id.clone(),
                    })
                }
                Err(_) => Err(AgentError::SessionLocked(format!(
                    "Another activation is currently running in this store (cannot run session {})",
                    session_id
                ))),
            }
        } else {
            let mut active = get_in_memory_active().lock().unwrap();
            if let Some(ref current) = *active {
                return Err(AgentError::SessionLocked(format!(
                    "Another activation ({}) is currently running in this store (cannot run session {})",
                    current, session_id
                )));
            }
            *active = Some(session_id.to_string());
            Ok(Self {
                _file: None,
                is_in_memory: true,
                session_id: session_id.clone(),
            })
        }
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

impl Drop for RunnerLock {
    fn drop(&mut self) {
        if self.is_in_memory {
            let mut active = get_in_memory_active().lock().unwrap();
            *active = None;
        }
        if let Some(file) = &self._file {
            let _ = file.unlock();
        }
    }
}
