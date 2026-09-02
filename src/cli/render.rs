use crate::control::runner::SessionRunResult;
use crate::core::context::extract_item_text;
use crate::domain::batch::ItemBatch;
use crate::domain::event::{SessionEvent, SessionEventEnvelope};
use crate::domain::session::{SessionSpec, SessionStatus};
use openresponses_rust::Item;

pub fn render_session_created(spec: &SessionSpec) {
    println!("成功创建 Session:");
    println!("  Session ID:   {}", spec.id);
    println!("  Workspace:    {}", spec.workspace_ref);
    println!("  Config Ref:   {}", spec.default_config_ref);
    println!("  Created At:   {}", spec.created_at.to_rfc3339());
}

pub fn render_session_list(sessions: &[(SessionSpec, SessionStatus)]) {
    if sessions.is_empty() {
        println!("暂无历史会话。");
        return;
    }

    println!(
        "{:<36} {:<10} {:<10} {:<10} {:<25}",
        "SESSION ID", "PHASE", "ITEMS", "BATCHES", "UPDATED AT"
    );
    println!("{:-<95}", "");

    for (spec, status) in sessions {
        let phase_str = match status.phase {
            crate::domain::session::SessionPhase::Open => "open",
            crate::domain::session::SessionPhase::Closed => "closed",
            crate::domain::session::SessionPhase::Archived => "archived",
            crate::domain::session::SessionPhase::Corrupted => "corrupted",
        };
        println!(
            "{:<36} {:<10} {:<10} {:<10} {:<25}",
            spec.id.as_str(),
            phase_str,
            status.local_item_count,
            status.batch_count,
            status.updated_at.format("%Y-%m-%d %H:%M:%S")
        );
    }
}

pub fn render_session_show(spec: &SessionSpec, status: &SessionStatus, context_items: &[Item]) {
    println!("=== 会话详情 ===");
    println!("ID:               {}", spec.id);
    println!("Workspace:        {}", spec.workspace_ref);
    println!("Config Ref:       {}", spec.default_config_ref);
    println!("Phase:            {:?}", status.phase);
    println!("Local Items:      {}", status.local_item_count);
    println!("Batches:          {}", status.batch_count);
    println!("Events:           {}", status.event_count);
    if let Some(ref err) = status.last_error {
        println!("Last Error:       {}", err);
    }
    println!("Created At:       {}", spec.created_at.to_rfc3339());
    println!("Updated At:       {}", status.updated_at.to_rfc3339());

    println!("\n=== 对话历史 (共 {} 条) ===", context_items.len());
    for (i, item) in context_items.iter().enumerate() {
        match item {
            Item::Message { role, .. } => {
                let text = extract_item_text(item);
                println!("\n[{}] {:?}:", i, role);
                println!("{}", text);
            }
            Item::Reasoning { .. } => {
                let thought = extract_item_text(item);
                println!("\n[{}] [Reasoning]:", i);
                println!("{}", thought);
            }
            Item::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                println!("\n[{}] [FunctionCall] id={}, tool={}:", i, call_id, name);
                println!("  args: {}", arguments);
            }
            Item::FunctionCallOutput {
                call_id, output, ..
            } => {
                println!("\n[{}] [ToolOutput] call_id={}:", i, call_id);
                println!("  output: {:?}", output);
            }
            _ => {
                println!("\n[{}] [Other Item]: {:?}", i, item);
            }
        }
    }
}

pub fn render_session_history(
    spec: &SessionSpec,
    events: &[SessionEventEnvelope],
    batches: &[ItemBatch],
) {
    println!("=== Session 历史事件与 Batch ===");
    println!("Session ID: {}\n", spec.id);

    println!("--- Events (共 {} 条) ---", events.len());
    for env in events {
        println!(
            "[{}] seq={} kind={} created={}",
            env.event_id,
            env.event_seq,
            env.event.kind_str(),
            env.created_at.format("%H:%M:%S%.3f")
        );
    }

    println!("\n--- Batches (共 {} 个) ---", batches.len());
    for b in batches {
        println!(
            "Batch #{}: kind={:?}, local_items=[{}..{}], items_count={}",
            b.batch_seq,
            b.kind,
            b.first_local_item_seq,
            b.last_local_item_seq(),
            b.items.len()
        );
    }
}

pub fn render_event_progress(env: &SessionEventEnvelope) {
    match &env.event {
        SessionEvent::SessionCreated { .. } => {
            println!("[Event] 会话已创建");
        }
        SessionEvent::ActivationRequested {
            input_batch_seq, ..
        } => {
            println!("[Event] 提交激活请求 (Input Batch #{})", input_batch_seq);
        }
        SessionEvent::ActivationStarted => {
            println!("[Event] 激活开始执行");
        }
        SessionEvent::TurnStarted => {
            println!("\n[Event] 开始思考与模型推理...");
        }
        SessionEvent::TurnCompleted { usage, .. } => {
            if let Some(u) = usage {
                println!(
                    "[Event] 模型推理完成 (耗费 Token: total={}, in={}, out={})",
                    u.total_tokens, u.input_tokens, u.output_tokens
                );
            } else {
                println!("[Event] 模型推理完成");
            }
        }
        SessionEvent::ToolCallStarted { name, call_id } => {
            println!("[Event] 调用工具: {} (call_id: {})", name, call_id);
        }
        SessionEvent::ToolCallFinished {
            name,
            success,
            duration_ms,
            ..
        } => {
            let dur = duration_ms.map(|d| format!("{}ms", d)).unwrap_or_default();
            println!(
                "[Event] 工具执行完成: {} (success: {}, {})",
                name, success, dur
            );
        }
        SessionEvent::ActivationCompleted { usage } => {
            println!("\n[Event] 激活已完成！");
            if let Some(u) = usage {
                println!(
                    "Token 统计: total={}, input={}, output={}",
                    u.total_tokens, u.input_tokens, u.output_tokens
                );
            }
        }
        SessionEvent::ActivationFailed { error } => {
            eprintln!("\n[Event] 激活失败: {}", error);
        }
        SessionEvent::ActivationCancelled => {
            println!("\n[Event] 激活已取消");
        }
        SessionEvent::ActivationInterrupted { reason } => {
            println!("\n[Event] 激活被中断: {}", reason);
        }
        _ => {}
    }
}

pub fn render_run_result(res: &SessionRunResult) {
    if !res.final_text.is_empty() {
        println!("\n=== 回答 ===");
        println!("{}", res.final_text);
    }
}
