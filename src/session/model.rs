use crate::error::AgentError;
use chrono::{DateTime, Local, Utc};
use openresponses_rust::Item;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_SYSTEM_PROMPT: &str = "你是一个高效、精准、善于深度思考的 AI 智能体助手";
pub const SESSION_SCHEMA_VERSION: u32 = 2;

/// 获取指定 session 名称的临时工作目录路径: /tmp/ragent/${session_name}
pub fn session_tmp_dir(session_name: &str) -> PathBuf {
    PathBuf::from("/tmp/ragent").join(session_name)
}

/// 校验 session ID / session 名称合法性（防止路径穿越等非法字符）
pub fn validate_session_id(id: &str) -> Result<(), AgentError> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(AgentError::InvalidSessionId(id.to_string()));
    }
    Ok(())
}

/// 确保指定 session 名称的临时工作目录已创建
pub fn ensure_session_tmp_dir(session_name: &str) -> Result<PathBuf, AgentError> {
    validate_session_id(session_name)?;
    let dir = session_tmp_dir(session_name);
    fs::create_dir_all(&dir).map_err(|e| {
        AgentError::ToolError(format!(
            "Failed to create session temp directory {:?}: {}",
            dir, e
        ))
    })?;
    Ok(dir)
}

/// 为新 Session 生成一次性的基础系统提示词。
pub fn build_basic_system_prompt(session_id: &str) -> Result<String, AgentError> {
    validate_session_id(session_id)?;
    let tmp_path = session_tmp_dir(session_id);
    Ok(format!(
        "{DEFAULT_SYSTEM_PROMPT}\n\n\
        ## 会话临时目录 (session_tmp)\n\
        当前会话临时目录为: `{}`\n\
        使用指南:\n\
        - 该目录用来放置那些不适合出现在 workspace 的文件，比如从剪切板粘贴的图片、测试代码等过渡内容。\n\
        - 在处理临时或过渡性文件时，请优先使用此目录，避免污染工作区。",
        tmp_path.display()
    ))
}

/// 会话元数据（用于列表快速展示与定位）
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    /// 会话唯一标识符 (如 "sess_1776158291_a1b2")
    pub id: String,
    /// 会话标题/摘要（自动从首条用户消息生成）
    pub title: String,
    /// 使用的大模型名称
    pub model: String,
    /// 创建时间戳（秒）
    pub created_at: u64,
    /// 最后更新时间戳（秒）
    pub updated_at: u64,
    /// 当前包含的消息与上下文项总数
    pub item_count: usize,
}

impl SessionMeta {
    /// 获取人类可读的本地格式化更新时间
    pub fn formatted_updated_at(&self) -> String {
        let naive = DateTime::from_timestamp(self.updated_at as i64, 0)
            .unwrap_or_else(|| DateTime::<Utc>::from(UNIX_EPOCH));
        let local: DateTime<Local> = DateTime::from(naive);
        local.format("%Y-%m-%d %H:%M:%S").to_string()
    }
}

/// 完整会话数据结构（用于文件持久化保存）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionData {
    pub schema_version: u32,
    pub meta: SessionMeta,
    /// Session 创建时由 ragent 固化，之后不再由核心更新。
    basic_system_prompt: String,
    pub items: Vec<Item>,
}

impl SessionData {
    /// 创建新的空会话
    pub fn new(id: impl Into<String>, model: impl Into<String>) -> Result<Self, AgentError> {
        let now = now_secs();
        let id_str = id.into();
        let basic_system_prompt = build_basic_system_prompt(&id_str)?;
        Ok(Self {
            schema_version: SESSION_SCHEMA_VERSION,
            meta: SessionMeta {
                id: id_str,
                title: "新建会话".to_string(),
                model: model.into(),
                created_at: now,
                updated_at: now,
                item_count: 0,
            },
            basic_system_prompt,
            items: Vec::new(),
        })
    }

    pub fn basic_system_prompt(&self) -> &str {
        &self.basic_system_prompt
    }

    pub fn validate_schema_version(&self) -> Result<(), AgentError> {
        if self.schema_version != SESSION_SCHEMA_VERSION {
            return Err(AgentError::UnsupportedSessionVersion {
                found: Some(u64::from(self.schema_version)),
                expected: SESSION_SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    /// 获取该 session 的临时工作目录路径 (/tmp/ragent/${session_id})
    pub fn temp_dir(&self) -> PathBuf {
        session_tmp_dir(&self.meta.id)
    }

    /// 确保该 session 的临时工作目录存在（若不存在则自动创建）
    pub fn ensure_temp_dir(&self) -> Result<PathBuf, AgentError> {
        ensure_session_tmp_dir(&self.meta.id)
    }

    /// 确保指定 session 名称的临时工作目录存在
    pub fn ensure_temp_dir_for(session_name: &str) -> Result<PathBuf, AgentError> {
        ensure_session_tmp_dir(session_name)
    }

    /// 自动生成一个带有时间戳和随机短码的 Session ID
    pub fn generate_id() -> String {
        let now = now_secs();
        let rand_val = (now ^ (now << 13)) % 0xffff;
        format!("sess_{:x}_{:04x}", now, rand_val)
    }

    /// 从当前上下文的 Items 更新 Session 数据与元数据
    pub fn update_from_context(&mut self, items: Vec<Item>) {
        self.items = items;
        self.meta.item_count = self.items.len();
        self.meta.updated_at = now_secs();

        // 提取第一条用户消息作为会话标题
        if self.meta.title == "新建会话" || self.meta.title.is_empty() {
            if let Some(first_user_msg) = self.find_first_user_message() {
                self.meta.title = Self::truncate_title(&first_user_msg, 40);
            }
        }
    }

    /// 截取标题
    fn truncate_title(content: &str, max_chars: usize) -> String {
        let clean = content.replace(['\r', '\n', '\t'], " ");
        let trimmed = clean.trim();
        let mut chars = trimmed.chars();
        let truncated: String = chars.by_ref().take(max_chars).collect();
        if chars.next().is_some() {
            format!("{}...", truncated)
        } else {
            truncated
        }
    }

    /// 查找首条用户输入文本
    fn find_first_user_message(&self) -> Option<String> {
        for item in &self.items {
            if let Ok(v) = serde_json::to_value(item) {
                let role = v.get("role").and_then(|r| r.as_str()).unwrap_or("");
                if role.eq_ignore_ascii_case("user") {
                    if let Some(c_str) = v.get("content").and_then(|c| c.as_str()) {
                        if !c_str.trim().is_empty() {
                            return Some(c_str.to_string());
                        }
                    } else if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
                        for part in arr {
                            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                                if !t.trim().is_empty() {
                                    return Some(t.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
