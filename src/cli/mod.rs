pub mod handler;

use crate::config::AgentConfig;
use crate::error::AgentError;
use handler::CliHandler;
use std::path::PathBuf;

pub enum CliCommand {
    Help,
    ListSessions,
    ViewSession {
        session_id: String,
    },
    DeleteSession {
        id_or_all: String,
    },
    ResumeSession {
        session_id: Option<String>,
        prompt: String,
    },
    RunNewSession {
        prompt: String,
    },
}

pub struct ParsedCli {
    pub custom_dir: Option<PathBuf>,
    pub custom_model: Option<String>,
    pub command: CliCommand,
}

pub fn print_help(program: &str) {
    println!("ragent - 极简 LLM Agent 命令行工具\n");
    println!("用法:");
    println!(
        "  {} \"<prompt>\" [-m <model>] [-d <dir>]                  新建会话并执行",
        program
    );
    println!(
        "  {} s list [-d <dir>]                                   查看历史会话列表",
        program
    );
    println!(
        "  {} s view <session_id> [-d <dir>]                      查看指定会话的对话历史",
        program
    );
    println!(
        "  {} s del <session_id> [-d <dir>]                       删除指定会话",
        program
    );
    println!(
        "  {} s del -a [-d <dir>]                                 清空所有历史会话",
        program
    );
    println!(
        "  {} s [session_id] \"<prompt>\" [-m <model>] [-d <dir>]   继续会话 (session_id 为空或省略则继续最近一次会话)",
        program
    );
    println!();
    println!("示例:");
    println!("  {} \"帮我查看当前目录下有哪些文件\"", program);
    println!("  {} \"写一个快速排序\" -m gemini-2.5-pro", program);
    println!("  {} s list", program);
    println!("  {} s view sess_1a2b_3c4d", program);
    println!(
        "  {} s \"接着刚才的结果，提取出所有的 .rs 文件\"  # 继续最近一次会话",
        program
    );
    println!(
        "  {} s \"\" \"接着刚才的结果继续\"                   # 显式传空 ID 继续最近一次会话",
        program
    );
    println!(
        "  {} s sess_1a2b_3c4d \"分析这些文件的依赖关系\" -m gpt-4o   # 继续指定 ID 会话并覆盖模型",
        program
    );
    println!("  {} s del sess_1a2b_3c4d", program);
    println!("  {} s del -a", program);
    println!();
    println!("选项:");
    println!("  -m, --model <model>  覆盖指定大模型名称 (优先级高于配置文件)");
    println!("  -d, --dir <dir>      指定会话存储目录 (默认: .ragent/sessions)");
    println!("  -h, --help           显示帮助信息");
    println!();
    println!("环境变量说明:");
    println!("  ROSETTA_URL        API Base URL (必须)");
    println!("  ROSETTA_TOKEN      API Key / Token (必须)");
    println!();
    println!("配置文件与优先级:");
    println!("  CLI 参数:    -m / --model 指定模型名称 (最高优先级)");
    println!("  项目配置:    当前工作目录下的 .ragent/config.toml (按叶子节点 key 覆盖全局配置)");
    println!("  全局配置:    ~/.config/ragent/config.toml");
    println!("  - 模型与思考参数: 通过 [model] 块配置 (如 name, temperature, reasoning 等)");
    println!("  - 项目扩展限制: 仅允许配置 name, enabled, config，不允许新增全局未声明扩展");
}

/// 解析命令行参数
pub fn parse_cli_args(args: &[String]) -> Result<ParsedCli, String> {
    if args.len() <= 1 {
        return Ok(ParsedCli {
            custom_dir: None,
            custom_model: None,
            command: CliCommand::Help,
        });
    }

    let mut custom_dir = None;
    let mut custom_model = None;
    let mut tokens = Vec::new();
    let mut i = 1;

    // 1. 提取选项参数 (-d/--dir, -m/--model, -h/--help)
    while i < args.len() {
        let arg = &args[i];
        if arg == "-d" || arg == "--dir" {
            if i + 1 < args.len() {
                custom_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
                continue;
            } else {
                return Err("选项 '-d' / '--dir' 需要提供目录路径参数".to_string());
            }
        } else if let Some(dir_str) = arg.strip_prefix("--dir=") {
            if dir_str.trim().is_empty() {
                return Err("选项 '-d' / '--dir' 需要提供目录路径参数".to_string());
            }
            custom_dir = Some(PathBuf::from(dir_str));
            i += 1;
            continue;
        } else if let Some(dir_str) = arg.strip_prefix("-d=") {
            if dir_str.trim().is_empty() {
                return Err("选项 '-d' / '--dir' 需要提供目录路径参数".to_string());
            }
            custom_dir = Some(PathBuf::from(dir_str));
            i += 1;
            continue;
        } else if arg == "-m" || arg == "--model" {
            if i + 1 < args.len() {
                let model_str = &args[i + 1];
                if model_str.trim().is_empty() {
                    return Err("选项 '-m' / '--model' 模型名称不能为空".to_string());
                }
                custom_model = Some(model_str.clone());
                i += 2;
                continue;
            } else {
                return Err("选项 '-m' / '--model' 需要提供模型名称参数".to_string());
            }
        } else if let Some(model_str) = arg.strip_prefix("--model=") {
            if model_str.trim().is_empty() {
                return Err("选项 '-m' / '--model' 模型名称不能为空".to_string());
            }
            custom_model = Some(model_str.to_string());
            i += 1;
            continue;
        } else if let Some(model_str) = arg.strip_prefix("-m=") {
            if model_str.trim().is_empty() {
                return Err("选项 '-m' / '--model' 模型名称不能为空".to_string());
            }
            custom_model = Some(model_str.to_string());
            i += 1;
            continue;
        } else if arg == "-h" || arg == "--help" {
            return Ok(ParsedCli {
                custom_dir,
                custom_model,
                command: CliCommand::Help,
            });
        } else {
            tokens.push(arg.clone());
            i += 1;
        }
    }

    if tokens.is_empty() {
        return Ok(ParsedCli {
            custom_dir,
            custom_model,
            command: CliCommand::Help,
        });
    }

    // 2. 判断是否进入 `s` 子命令集
    if tokens[0] == "s" || tokens[0] == "session" {
        if tokens.len() == 1 {
            // `ragent s` 默认列出 session
            return Ok(ParsedCli {
                custom_dir,
                custom_model,
                command: CliCommand::ListSessions,
            });
        }

        match tokens[1].as_str() {
            "list" | "ls" => Ok(ParsedCli {
                custom_dir,
                custom_model,
                command: CliCommand::ListSessions,
            }),
            "view" | "show" => {
                if tokens.len() < 3 {
                    Err("用法: ragent s view <session_id>".to_string())
                } else {
                    Ok(ParsedCli {
                        custom_dir,
                        custom_model,
                        command: CliCommand::ViewSession {
                            session_id: tokens[2].clone(),
                        },
                    })
                }
            }
            "del" | "rm" | "delete" => {
                if tokens.len() < 3 {
                    Err("用法: ragent s del <session_id> 或 ragent s del -a".to_string())
                } else {
                    Ok(ParsedCli {
                        custom_dir,
                        custom_model,
                        command: CliCommand::DeleteSession {
                            id_or_all: tokens[2].clone(),
                        },
                    })
                }
            }
            _ => {
                // tokens[0] == "s"
                // 1. 检查 tokens[1] 是否为空字符串，如 `ragent s "" "prompt"`
                if tokens[1].trim().is_empty() {
                    let prompt = if tokens.len() > 2 {
                        tokens[2..].join(" ")
                    } else {
                        "".to_string()
                    };
                    if prompt.is_empty() {
                        return Ok(ParsedCli {
                            custom_dir,
                            custom_model,
                            command: CliCommand::ListSessions,
                        });
                    }
                    Ok(ParsedCli {
                        custom_dir,
                        custom_model,
                        command: CliCommand::ResumeSession {
                            session_id: None,
                            prompt,
                        },
                    })
                } else if tokens.len() == 2 {
                    // `ragent s "<prompt>"` -> session_id 省略，继续最近一次会话
                    Ok(ParsedCli {
                        custom_dir,
                        custom_model,
                        command: CliCommand::ResumeSession {
                            session_id: None,
                            prompt: tokens[1].clone(),
                        },
                    })
                } else {
                    // tokens.len() >= 3: 如 `ragent s sess_123 "prompt"`
                    let possible_id = &tokens[1];
                    if possible_id.starts_with("sess_")
                        || (!possible_id.contains(' ') && possible_id.len() <= 32)
                    {
                        let prompt = tokens[2..].join(" ");
                        Ok(ParsedCli {
                            custom_dir,
                            custom_model,
                            command: CliCommand::ResumeSession {
                                session_id: Some(possible_id.clone()),
                                prompt,
                            },
                        })
                    } else {
                        let prompt = tokens[1..].join(" ");
                        Ok(ParsedCli {
                            custom_dir,
                            custom_model,
                            command: CliCommand::ResumeSession {
                                session_id: None,
                                prompt,
                            },
                        })
                    }
                }
            }
        }
    } else {
        // 直接执行新建 session: `ragent "<prompt>"`
        let prompt = tokens.join(" ");
        Ok(ParsedCli {
            custom_dir,
            custom_model,
            command: CliCommand::RunNewSession { prompt },
        })
    }
}

/// 执行 CLI 流程入口
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

    let handler = CliHandler::new(parsed.custom_dir, config);

    match parsed.command {
        CliCommand::Help => {
            print_help(&program);
        }
        CliCommand::ListSessions => {
            handler.list_sessions()?;
        }
        CliCommand::ViewSession { session_id } => {
            handler.view_session(&session_id)?;
        }
        CliCommand::DeleteSession { id_or_all } => {
            handler.delete_session(&id_or_all)?;
        }
        CliCommand::ResumeSession { session_id, prompt } => {
            handler.run_or_resume(session_id, &prompt).await?;
        }
        CliCommand::RunNewSession { prompt } => {
            let new_id = crate::session::SessionData::generate_id();
            handler.run_or_resume(Some(new_id), &prompt).await?;
        }
    }

    Ok(())
}
