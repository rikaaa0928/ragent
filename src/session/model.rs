use chrono::{DateTime, Local, Utc};
use openresponses_rust::Item;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

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
    pub meta: SessionMeta,
    pub system_prompt: Option<String>,
    pub items: Vec<Item>,
}

impl SessionData {
    /// 创建新的空会话
    pub fn new(
        id: impl Into<String>,
        model: impl Into<String>,
        system_prompt: Option<String>,
    ) -> Self {
        let now = now_secs();
        let id_str = id.into();
        Self {
            meta: SessionMeta {
                id: id_str,
                title: "新建会话".to_string(),
                model: model.into(),
                created_at: now,
                updated_at: now,
                item_count: 0,
            },
            system_prompt,
            items: Vec::new(),
        }
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

    /// 计算会话历史中已完成的模型响应轮次
    pub fn history_turns(&self) -> usize {
        self.items
            .iter()
            .filter(|item| match item {
                Item::Message { role, .. } => format!("{role:?}").eq_ignore_ascii_case("assistant"),
                _ => false,
            })
            .count()
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
