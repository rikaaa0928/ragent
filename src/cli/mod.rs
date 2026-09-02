pub mod command;
pub mod render;

use crate::config::AgentConfig;
use crate::control::service::ControlService;
use crate::domain::ids::SessionId;
use crate::error::AgentError;
pub use command::{parse_cli_args, print_help, CliCommand, ParsedCli};
pub use render::*;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn resolve_store_path(custom_dir: Option<PathBuf>) -> PathBuf {
    match custom_dir {
        Some(dir) => {
            if dir.extension().and_then(|s| s.to_str()) == Some("sqlite3")
                || dir.extension().and_then(|s| s.to_str()) == Some("db")
            {
                dir
            } else {
                dir.join("control.sqlite3")
            }
        }
        None => PathBuf::from(".ragent/store/control.sqlite3"),
    }
}

pub async fn run_cli(args: &[String], mut config: AgentConfig) -> Result<(), AgentError> {
    let program = args
        .first()
        .cloned()
        .unwrap_or_else(|| "ragent".to_string());
    let parsed = match parse_cli_args(args) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("参数错误: {}", err);
            print_help(&program);
            return Ok(());
        }
    };

    if let Some(ref model) = parsed.custom_model {
        config = config.with_model(model);
    }

    let store_path = resolve_store_path(parsed.custom_dir);
    let service = ControlService::open(store_path, config)?;

    match parsed.command {
        CliCommand::Help => {
            print_help(&program);
        }
        CliCommand::SessionCreate { workspace, prompt } => {
            let spec = service.create_session(&workspace, prompt.as_deref(), None)?;
            render_session_created(&spec);
        }
        CliCommand::SessionList => {
            let list = service.list_sessions()?;
            render_session_list(&list);
        }
        CliCommand::SessionShow { session_id } => {
            let id = SessionId::new(session_id);
            let (spec, status) = service
                .get_session(&id)?
                .ok_or_else(|| AgentError::InvalidSessionId(id.to_string()))?;
            let items = service.read_context(&id)?;
            render_session_show(&spec, &status, &items);
        }
        CliCommand::SessionHistory { session_id } => {
            let id = SessionId::new(session_id);
            let (spec, _) = service
                .get_session(&id)?
                .ok_or_else(|| AgentError::InvalidSessionId(id.to_string()))?;
            let events = service.read_events(&id)?;
            let batches = service
                .store()
                .read_batches(&id)
                .map_err(|e| AgentError::ToolError(e.to_string()))?;
            render_session_history(&spec, &events, &batches);
        }
        CliCommand::SessionRun { session_id, input } => {
            let id = SessionId::new(session_id);
            let cancellation = CancellationToken::new();
            let token_for_ctrlc = cancellation.clone();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    println!("\n[收到中断信号，正在取消当前 Activation...]");
                    token_for_ctrlc.cancel();
                }
            });

            let event_cb = Arc::new(|env: &crate::domain::event::SessionEventEnvelope| {
                render_event_progress(env);
            });

            match service
                .run_session(&id, &input, cancellation, Some(event_cb))
                .await
            {
                Ok(result) => render_run_result(&result),
                Err(AgentError::Cancelled) => {
                    println!("\n[已成功取消当前 Activation]");
                }
                Err(e) => return Err(e),
            }
        }
        CliCommand::RunNew { workspace, prompt } => {
            let ws = workspace.unwrap_or_else(|| PathBuf::from("."));
            let spec = service.create_session(&ws, None, None)?;
            println!("已自动创建会话: {}", spec.id);

            let cancellation = CancellationToken::new();
            let token_for_ctrlc = cancellation.clone();
            tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    println!("\n[收到中断信号，正在取消当前 Activation...]");
                    token_for_ctrlc.cancel();
                }
            });

            let event_cb = Arc::new(|env: &crate::domain::event::SessionEventEnvelope| {
                render_event_progress(env);
            });

            match service
                .run_session(&spec.id, &prompt, cancellation, Some(event_cb))
                .await
            {
                Ok(result) => render_run_result(&result),
                Err(AgentError::Cancelled) => {
                    println!("\n[已成功取消当前 Activation]");
                }
                Err(e) => return Err(e),
            }
        }
    }

    Ok(())
}
