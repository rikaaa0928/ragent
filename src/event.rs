use openresponses_rust::Usage;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::ops::AddAssign;
use std::sync::{Arc, Mutex};

/// 对话/请求的 Token 用量统计
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    /// 输入 Token 数
    pub input_tokens: i32,
    /// 输出 Token 数
    pub output_tokens: i32,
    /// 总 Token 数
    pub total_tokens: i32,
    /// 缓存命中的输入 Token 数
    pub cached_tokens: i32,
    /// 思考/推理生成的 Token 数
    pub reasoning_tokens: i32,
}

impl TokenUsage {
    pub fn new(
        input_tokens: i32,
        output_tokens: i32,
        total_tokens: i32,
        cached_tokens: i32,
        reasoning_tokens: i32,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            cached_tokens,
            reasoning_tokens,
        }
    }

    /// 格式化为展示字符串，例如："总计: 1500 (输入: 1000, 输出: 500, 缓存: 800, 思考: 200)"
    pub fn formatted_details(&self) -> String {
        format!(
            "总计: {} (输入: {}, 输出: {}, 缓存: {}, 思考: {})",
            self.total_tokens,
            self.input_tokens,
            self.output_tokens,
            self.cached_tokens,
            self.reasoning_tokens
        )
    }
}

impl AddAssign<&TokenUsage> for TokenUsage {
    fn add_assign(&mut self, rhs: &TokenUsage) {
        self.input_tokens += rhs.input_tokens;
        self.output_tokens += rhs.output_tokens;
        self.total_tokens += rhs.total_tokens;
        self.cached_tokens += rhs.cached_tokens;
        self.reasoning_tokens += rhs.reasoning_tokens;
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, rhs: Self) {
        *self += &rhs;
    }
}

impl From<&Usage> for TokenUsage {
    fn from(usage: &Usage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            cached_tokens: usage.input_tokens_details.cached_tokens,
            reasoning_tokens: usage.output_tokens_details.reasoning_tokens,
        }
    }
}

impl From<Usage> for TokenUsage {
    fn from(usage: Usage) -> Self {
        Self::from(&usage)
    }
}

/// Agent 运行周期内派发的事件
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// 收到输入消息
    MessageReceived { content: String, is_delayed: bool },
    /// 单轮循环/推理开始
    TurnStarted { iteration: usize },
    /// 单轮模型响应完成
    TurnCompleted {
        iteration: usize,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
    },
    /// 开始调用工具
    ToolCallStarted {
        call_id: String,
        tool_name: String,
        arguments: String,
    },
    /// 工具执行完成
    ToolCallFinished {
        call_id: String,
        tool_name: String,
        output: String,
        is_error: bool,
        duration_ms: u128,
    },
    /// 本轮对话与工具链执行完毕
    RoundCompleted {
        iteration: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_usage: Option<TokenUsage>,
    },
    /// Agent 全部任务结束并退出
    AgentFinished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_usage: Option<TokenUsage>,
    },
    /// 运行过程中发生的错误
    Error { error: String },
}

/// 事件处理接口（观察者模式）
pub trait EventHandler: Send + Sync {
    fn on_event(&self, event: &AgentEvent);
}

/// 默认的控制台事件处理器（人类可读文本格式）
pub struct ConsoleEventHandler {
    pub show_tool_output: bool,
}

impl ConsoleEventHandler {
    pub fn new() -> Self {
        Self {
            show_tool_output: true,
        }
    }
}

impl Default for ConsoleEventHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHandler for ConsoleEventHandler {
    fn on_event(&self, event: &AgentEvent) {
        match event {
            AgentEvent::MessageReceived {
                content,
                is_delayed,
            } => {
                let tag = if *is_delayed {
                    "[延时消息]"
                } else {
                    "[及时消息]"
                };
                println!("\n>>> 收到 {}: {}", tag, content);
            }
            AgentEvent::TurnStarted { iteration } => {
                println!("\n--- [Agent 轮次 {}] 模型思考/回复中 ---", iteration);
            }
            AgentEvent::TurnCompleted {
                text,
                reasoning,
                usage,
                ..
            } => {
                if let Some(reasoning) = reasoning {
                    let trimmed = reasoning.trim();
                    if !trimmed.is_empty() {
                        println!("\n--- [思考摘要] ---:\n{}\n--- [摘要结束] ---", trimmed);
                    }
                }
                if !text.is_empty() {
                    println!("{}", text);
                } else {
                    println!();
                }
                if let Some(usage) = usage {
                    println!("[Token 用量] {}", usage.formatted_details());
                }
            }
            AgentEvent::ToolCallStarted {
                tool_name,
                arguments,
                ..
            } => {
                println!(
                    "\n[Tool 准备执行] -> 工具: {} | 参数: {}",
                    tool_name,
                    arguments.trim()
                );
            }
            AgentEvent::ToolCallFinished {
                tool_name,
                output,
                is_error,
                duration_ms,
                ..
            } => {
                let status = if *is_error { "失败" } else { "成功" };
                println!(
                    "[Tool 执行完成] -> 工具: {} ({} - {}ms)",
                    tool_name, status, duration_ms
                );
                if self.show_tool_output {
                    match output.char_indices().nth(300) {
                        Some((idx, _)) => {
                            println!(
                                "[Tool 输出预览]:\n{}... (共 {} 字符)",
                                output[..idx].trim(),
                                output.chars().count()
                            );
                        }
                        None => {
                            println!("[Tool 输出预览]:\n{}", output.trim());
                        }
                    }
                }
            }
            AgentEvent::RoundCompleted {
                iteration,
                total_usage,
                ..
            } => {
                println!("=== [轮次 {} 完成] ===", iteration);
                if let Some(total) = total_usage {
                    println!("[Token 累计] {}\n", total.formatted_details());
                } else {
                    println!();
                }
            }
            AgentEvent::AgentFinished { total_usage } => {
                println!("\n=== Agent 执行流程结束 ===");
                if let Some(total) = total_usage {
                    println!("[Token 总量] {}", total.formatted_details());
                }
            }
            AgentEvent::Error { error } => {
                eprintln!("\n[Agent 错误]: {}", error);
            }
        }
    }
}

/// JSON Lines (JSONL) 格式事件处理器，每一行输出一个序列化后的 JSON 事件对象
pub struct JsonLinesEventHandler {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl JsonLinesEventHandler {
    /// 创建输出到标准输出 (stdout) 的 JSONL 处理器
    pub fn stdout() -> Self {
        Self::new(Box::new(io::stdout()))
    }

    /// 创建输出到标准错误 (stderr) 的 JSONL 处理器
    pub fn stderr() -> Self {
        Self::new(Box::new(io::stderr()))
    }

    /// 创建输出到任意 `Write + Send` 实现的 JSONL 处理器（如文件、内存缓冲区等）
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
        }
    }
}

impl Default for JsonLinesEventHandler {
    fn default() -> Self {
        Self::stdout()
    }
}

impl EventHandler for JsonLinesEventHandler {
    fn on_event(&self, event: &AgentEvent) {
        if let Ok(json_line) = serde_json::to_string(event) {
            if let Ok(mut writer) = self.writer.lock() {
                let _ = writeln!(writer, "{}", json_line);
                let _ = writer.flush();
            }
        }
    }
}

/// 空事件处理器（静音模式）
pub struct NoopEventHandler;

impl EventHandler for NoopEventHandler {
    fn on_event(&self, _event: &AgentEvent) {}
}

/// 支持闭包形式的自定义事件处理器
pub struct FnEventHandler<F>(pub F)
where
    F: Fn(&AgentEvent) + Send + Sync;

impl<F> EventHandler for FnEventHandler<F>
where
    F: Fn(&AgentEvent) + Send + Sync,
{
    fn on_event(&self, event: &AgentEvent) {
        (self.0)(event);
    }
}
