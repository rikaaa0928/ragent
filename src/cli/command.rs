use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum CliCommand {
    Help,
    SessionCreate {
        workspace: PathBuf,
        prompt: Option<String>,
    },
    SessionList,
    SessionShow {
        session_id: String,
    },
    SessionHistory {
        session_id: String,
    },
    SessionRun {
        session_id: String,
        input: String,
    },
    RunNew {
        workspace: Option<PathBuf>,
        prompt: String,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParsedCli {
    pub custom_dir: Option<PathBuf>,
    pub custom_model: Option<String>,
    pub command: CliCommand,
}

pub fn print_help(program: &str) {
    println!("ragent - LLM Agent 命令行工具\n");
    println!("用法:");
    println!(
        "  {} session create --workspace <path> [--prompt <text>]  新建会话",
        program
    );
    println!(
        "  {} session list                                         查看会话列表",
        program
    );
    println!(
        "  {} session show <session-id>                            查看会话详情",
        program
    );
    println!(
        "  {} session history <session-id>                         查看会话事实历史",
        program
    );
    println!(
        "  {} session run <session-id> \"<input>\"                   向会话提交输入并执行",
        program
    );
    println!(
        "  {} \"<input>\" [-m <model>] [-w <workspace>]              快速新建会话并执行",
        program
    );
    println!();
    println!("快捷别名 (支持 session / s):");
    println!("  {} s list", program);
    println!("  {} s show <session-id>", program);
    println!("  {} s history <session-id>", program);
    println!("  {} s run <session-id> \"<input>\"", program);
    println!("  {} s <session-id> \"<input>\"", program);
    println!();
    println!("选项:");
    println!("  -w, --workspace <path>  指定 Workspace 根目录 (默认: 当前工作目录)");
    println!("  -m, --model <model>      覆盖大模型名称");
    println!("  -d, --dir <store_dir>    指定 SQLite 存储目录 (默认: .ragent/store)");
    println!("  -h, --help               显示帮助信息");
}

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
    let mut custom_workspace = None;
    let mut custom_prompt = None;
    let mut tokens = Vec::new();
    let mut i = 1;

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
        } else if arg == "-w" || arg == "--workspace" {
            if i + 1 < args.len() {
                custom_workspace = Some(PathBuf::from(&args[i + 1]));
                i += 2;
                continue;
            } else {
                return Err("选项 '-w' / '--workspace' 需要提供路径参数".to_string());
            }
        } else if let Some(ws_str) = arg.strip_prefix("--workspace=") {
            if ws_str.trim().is_empty() {
                return Err("选项 '-w' / '--workspace' 需要提供路径参数".to_string());
            }
            custom_workspace = Some(PathBuf::from(ws_str));
            i += 1;
            continue;
        } else if let Some(ws_str) = arg.strip_prefix("-w=") {
            if ws_str.trim().is_empty() {
                return Err("选项 '-w' / '--workspace' 需要提供路径参数".to_string());
            }
            custom_workspace = Some(PathBuf::from(ws_str));
            i += 1;
            continue;
        } else if arg == "-p" || arg == "--prompt" {
            if i + 1 < args.len() {
                custom_prompt = Some(args[i + 1].clone());
                i += 2;
                continue;
            } else {
                return Err("选项 '-p' / '--prompt' 需要提供文本参数".to_string());
            }
        } else if let Some(prompt_str) = arg.strip_prefix("--prompt=") {
            custom_prompt = Some(prompt_str.to_string());
            i += 1;
            continue;
        } else if let Some(prompt_str) = arg.strip_prefix("-p=") {
            custom_prompt = Some(prompt_str.to_string());
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

    if tokens[0] == "session" || tokens[0] == "s" {
        if tokens.len() == 1 {
            return Ok(ParsedCli {
                custom_dir,
                custom_model,
                command: CliCommand::SessionList,
            });
        }

        match tokens[1].as_str() {
            "create" | "new" => {
                let ws = custom_workspace.unwrap_or_else(|| PathBuf::from("."));
                Ok(ParsedCli {
                    custom_dir,
                    custom_model,
                    command: CliCommand::SessionCreate {
                        workspace: ws,
                        prompt: custom_prompt,
                    },
                })
            }
            "list" | "ls" => Ok(ParsedCli {
                custom_dir,
                custom_model,
                command: CliCommand::SessionList,
            }),
            "show" | "view" => {
                if tokens.len() < 3 {
                    Err("用法: ragent session show <session_id>".to_string())
                } else {
                    Ok(ParsedCli {
                        custom_dir,
                        custom_model,
                        command: CliCommand::SessionShow {
                            session_id: tokens[2].clone(),
                        },
                    })
                }
            }
            "history" | "hist" => {
                if tokens.len() < 3 {
                    Err("用法: ragent session history <session_id>".to_string())
                } else {
                    Ok(ParsedCli {
                        custom_dir,
                        custom_model,
                        command: CliCommand::SessionHistory {
                            session_id: tokens[2].clone(),
                        },
                    })
                }
            }
            "run" => {
                if tokens.len() < 4 {
                    Err("用法: ragent session run <session_id> <input>".to_string())
                } else {
                    let session_id = tokens[2].clone();
                    let input = tokens[3..].join(" ");
                    Ok(ParsedCli {
                        custom_dir,
                        custom_model,
                        command: CliCommand::SessionRun { session_id, input },
                    })
                }
            }
            other_id => {
                // Shorthand: ragent s <session_id> <input>
                if tokens.len() >= 3 {
                    let input = tokens[2..].join(" ");
                    Ok(ParsedCli {
                        custom_dir,
                        custom_model,
                        command: CliCommand::SessionRun {
                            session_id: other_id.to_string(),
                            input,
                        },
                    })
                } else {
                    // Shorthand: ragent s <input> -> RunNew
                    let input = tokens[1..].join(" ");
                    Ok(ParsedCli {
                        custom_dir,
                        custom_model,
                        command: CliCommand::RunNew {
                            workspace: custom_workspace,
                            prompt: input,
                        },
                    })
                }
            }
        }
    } else {
        let prompt = tokens.join(" ");
        Ok(ParsedCli {
            custom_dir,
            custom_model,
            command: CliCommand::RunNew {
                workspace: custom_workspace,
                prompt,
            },
        })
    }
}
