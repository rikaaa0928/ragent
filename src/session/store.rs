use crate::error::AgentError;
use crate::session::model::{
    validate_session_id, SessionData, SessionMeta, SESSION_SCHEMA_VERSION,
};
use std::fs;
use std::path::{Path, PathBuf};

/// 基于文件目录的 Session 持久化存储管理器
#[derive(Debug, Clone)]
pub struct SessionStore {
    base_dir: PathBuf,
}

impl SessionStore {
    /// 创建新的 Session 存储器
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// 默认会话存储目录：当前工作目录下的 `.ragent/sessions`
    pub fn default_dir() -> PathBuf {
        PathBuf::from(".ragent/sessions")
    }

    /// 获取存储目录路径
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// 确保会话存储目录存在
    pub fn ensure_dir(&self) -> Result<(), AgentError> {
        if !self.base_dir.exists() {
            fs::create_dir_all(&self.base_dir).map_err(|e| {
                AgentError::ToolError(format!(
                    "Failed to create session directory {:?}: {}",
                    self.base_dir, e
                ))
            })?;
        }
        Ok(())
    }

    /// 获取特定 session 的 json 文件完整路径
    pub fn session_file_path(&self, id: &str) -> Result<PathBuf, AgentError> {
        validate_session_id(id)?;
        Ok(self.base_dir.join(format!("{id}.json")))
    }

    /// 保存会话数据（采用 Write-to-temp + Rename 原子写入策略）
    pub fn save(&self, session: &SessionData) -> Result<(), AgentError> {
        session.validate_schema_version()?;
        self.ensure_dir()?;
        let target_path = self.session_file_path(&session.meta.id)?;
        let temp_path = target_path.with_extension("json.tmp");

        let json_str = serde_json::to_string_pretty(session).map_err(AgentError::JsonError)?;

        fs::write(&temp_path, json_str).map_err(|e| {
            AgentError::ToolError(format!(
                "Failed to write temp session file {:?}: {}",
                temp_path, e
            ))
        })?;

        fs::rename(&temp_path, &target_path).map_err(|e| {
            AgentError::ToolError(format!(
                "Failed to commit session file rename {:?} -> {:?}: {}",
                temp_path, target_path, e
            ))
        })?;

        Ok(())
    }

    /// 加载指定 ID 的会话
    pub fn load(&self, id: &str) -> Result<Option<SessionData>, AgentError> {
        let file_path = self.session_file_path(id)?;
        if !file_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&file_path).map_err(|e| {
            AgentError::ToolError(format!(
                "Failed to read session file {:?}: {}",
                file_path, e
            ))
        })?;

        let session = Self::deserialize_session(&content, &file_path)?;

        Ok(Some(session))
    }

    /// 获取所有会话列表（按最后更新时间 updated_at 倒序排序）
    pub fn list(&self) -> Result<Vec<SessionMeta>, AgentError> {
        if !self.base_dir.exists() {
            return Ok(Vec::new());
        }

        let mut metas = Vec::new();
        let entries = fs::read_dir(&self.base_dir).map_err(|e| {
            AgentError::ToolError(format!(
                "Failed to read session dir {:?}: {}",
                self.base_dir, e
            ))
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(session) = Self::deserialize_session(&content, &path) {
                        metas.push(session.meta);
                    }
                }
            }
        }

        // 按 updated_at 降序排列（最新的在最前面）
        metas.sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        Ok(metas)
    }

    /// 获取最近活跃（最后更新）的一个会话
    pub fn load_latest(&self) -> Result<Option<SessionData>, AgentError> {
        let metas = self.list()?;
        if let Some(first_meta) = metas.first() {
            self.load(&first_meta.id)
        } else {
            Ok(None)
        }
    }

    /// 删除指定 ID 的会话文件
    pub fn delete(&self, id: &str) -> Result<bool, AgentError> {
        let file_path = self.session_file_path(id)?;
        if file_path.exists() {
            fs::remove_file(&file_path).map_err(|e| {
                AgentError::ToolError(format!(
                    "Failed to delete session file {:?}: {}",
                    file_path, e
                ))
            })?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 删除所有会话文件，返回删除的文件数量
    pub fn delete_all(&self) -> Result<usize, AgentError> {
        if !self.base_dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        let entries = fs::read_dir(&self.base_dir).map_err(|e| {
            AgentError::ToolError(format!(
                "Failed to read session dir {:?}: {}",
                self.base_dir, e
            ))
        })?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().and_then(|s| s.to_str()) == Some("json")
                && fs::remove_file(&path).is_ok()
            {
                count += 1;
            }
        }

        Ok(count)
    }

    fn deserialize_session(content: &str, path: &Path) -> Result<SessionData, AgentError> {
        let value: serde_json::Value = serde_json::from_str(content).map_err(|error| {
            AgentError::ToolError(format!("Corrupted session file {:?}: {}", path, error))
        })?;
        let found = value.get("schema_version").and_then(|value| value.as_u64());
        if found != Some(u64::from(SESSION_SCHEMA_VERSION)) {
            return Err(AgentError::UnsupportedSessionVersion {
                found,
                expected: SESSION_SCHEMA_VERSION,
            });
        }
        serde_json::from_value(value).map_err(|error| {
            AgentError::ToolError(format!("Corrupted session file {:?}: {}", path, error))
        })
    }
}
