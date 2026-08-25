use crate::builder::AgentBuilder;
use crate::config::AgentConfig;
use crate::error::AgentError;
use crate::event::ConsoleEventHandler;
use crate::session::{SessionData, SessionStore};
use std::path::PathBuf;
use std::sync::Arc;

pub struct CliHandler {
    store: SessionStore,
    config: AgentConfig,
}

impl CliHandler {
    pub fn new(custom_dir: Option<PathBuf>, config: AgentConfig) -> Self {
        let base_dir = custom_dir.unwrap_or_else(SessionStore::default_dir);
        Self {
            store: SessionStore::new(base_dir),
            config,
        }
    }

    /// 列出所有历史会话
    pub fn list_sessions(&self) -> Result<(), AgentError> {
        let list = self.store.list()?;
        if list.is_empty() {
            println!("暂无历史会话记录。使用 `ragent \"<prompt>\"` 开启新会话。");
            return Ok(());
        }

        println!("历史会话列表 (共 {} 条):", list.len());
        println!(
            "{:<24} | {:<19} | {:<6} | 摘要/标题",
            "Session ID", "更新时间", "消息数"
        );
        println!("{}", "-".repeat(75));

        for meta in list {
            println!(
                "{:<24} | {:<19} | {:<6} | {}",
                meta.id,
                meta.formatted_updated_at(),
                meta.item_count,
                meta.title
            );
        }
        Ok(())
    }

    /// 查看指定会话的完整历史
    pub fn view_session(&self, session_id: &str) -> Result<(), AgentError> {
        let session = match self.store.load(session_id)? {
            Some(s) => s,
            None => {
                println!("未找到 ID 为 '{}' 的会话。", session_id);
                return Ok(());
            }
        };

        println!("=== 会话详情: {} ===", session.meta.id);
        println!("标题: {}", session.meta.title);
        println!("模型: {}", session.meta.model);
        println!("更新时间: {}", session.meta.formatted_updated_at());
        println!("上下文项数: {}", session.meta.item_count);
        println!("--------------------------------------------------");

        for (i, item) in session.items.iter().enumerate() {
            let item_json = serde_json::to_string_pretty(item).unwrap_or_default();
            println!("[条目 #{}]\n{}", i + 1, item_json);
        }

        Ok(())
    }

    /// 删除指定会话或清空全部
    pub fn delete_session(&self, id_or_all: &str) -> Result<(), AgentError> {
        if id_or_all == "-a" || id_or_all == "--all" {
            let count = self.store.delete_all()?;
            println!("已清空所有会话 (共删除 {} 个会话文件)。", count);
        } else {
            let success = self.store.delete(id_or_all)?;
            if success {
                println!("已成功删除会话 '{}'。", id_or_all);
            } else {
                println!("未找到会话 '{}'，无需删除。", id_or_all);
            }
        }
        Ok(())
    }

    /// 执行或继续会话
    pub async fn run_or_resume(
        &self,
        session_id: Option<String>,
        initial_prompt: &str,
    ) -> Result<(), AgentError> {
        // 1. 加载或新建 SessionData
        let mut session_data = match session_id {
            Some(ref id) => {
                if let Some(loaded) = self.store.load(id)? {
                    println!(
                        "[会话已恢复] 正在继续会话 '{}' (标题: {})",
                        loaded.meta.id, loaded.meta.title
                    );
                    loaded
                } else {
                    println!("[新建会话] 创建会话 ID: {}", id);
                    SessionData::new(id, &self.config.model, None)
                }
            }
            None => {
                if let Some(latest) = self.store.load_latest()? {
                    println!(
                        "[继续最近会话] 正在继续会话 '{}' (标题: {})",
                        latest.meta.id, latest.meta.title
                    );
                    latest
                } else {
                    let new_id = SessionData::generate_id();
                    println!("[新建会话] 未找到历史会话，创建新会话: {}", new_id);
                    SessionData::new(new_id, &self.config.model, None)
                }
            }
        };

        let event_handler = Arc::new(ConsoleEventHandler::new());

        let (mut agent, _sender) =
            AgentBuilder::from_session(session_data.clone(), self.config.clone()).await?;

        agent.set_event_handler(event_handler);

        // 如果用户提供了 prompt，追加用户消息
        if !initial_prompt.trim().is_empty() {
            agent.add_user_message(initial_prompt);
        }

        // 运行 Agent
        let run_result = agent.run().await;
        let shutdown_result = agent.shutdown().await;

        // 保存会话状态（无论 run 是否产生错误都尽可能保存历史上下文）
        session_data.system_prompt = agent.context().system_prompt().map(str::to_owned);
        session_data.update_from_context(agent.context().to_items());
        self.store.save(&session_data)?;

        run_result?;
        shutdown_result
    }
}
