use openresponses_rust::ReasoningEffort;
use ragent::cli::{parse_cli_args, CliCommand};
use ragent::{AgentConfig, HookManager};
use std::path::PathBuf;

#[test]
fn cli_parses_model_override_options() {
    // 1. -m <model>
    let args = vec![
        "ragent".into(),
        "帮我写个脚本".into(),
        "-m".into(),
        "gpt-4o".into(),
    ];
    let parsed = parse_cli_args(&args).unwrap();
    assert_eq!(parsed.custom_model.as_deref(), Some("gpt-4o"));
    match parsed.command {
        CliCommand::RunNew { prompt, .. } => assert_eq!(prompt, "帮我写个脚本"),
        _ => panic!("unexpected command"),
    }

    // 2. -m before prompt
    let args = vec![
        "ragent".into(),
        "-m".into(),
        "gpt-4o-mini".into(),
        "写一个快速排序".into(),
    ];
    let parsed = parse_cli_args(&args).unwrap();
    assert_eq!(parsed.custom_model.as_deref(), Some("gpt-4o-mini"));
    match parsed.command {
        CliCommand::RunNew { prompt, .. } => assert_eq!(prompt, "写一个快速排序"),
        _ => panic!("unexpected command"),
    }

    // 3. --model <model>
    let args = vec![
        "ragent".into(),
        "--model".into(),
        "claude-3-5-sonnet".into(),
        "分析当前目录".into(),
    ];
    let parsed = parse_cli_args(&args).unwrap();
    assert_eq!(parsed.custom_model.as_deref(), Some("claude-3-5-sonnet"));

    // 4. --model=val and -m=val
    let args = vec![
        "ragent".into(),
        "--model=gemini-2.5-pro".into(),
        "测试".into(),
    ];
    let parsed = parse_cli_args(&args).unwrap();
    assert_eq!(parsed.custom_model.as_deref(), Some("gemini-2.5-pro"));

    let args = vec!["ragent".into(), "-m=deepseek-r1".into(), "测试".into()];
    let parsed = parse_cli_args(&args).unwrap();
    assert_eq!(parsed.custom_model.as_deref(), Some("deepseek-r1"));

    // 5. s run with -m and -d
    let args = vec![
        "ragent".into(),
        "s".into(),
        "sess_12345".into(),
        "继续".into(),
        "-m".into(),
        "custom-model".into(),
        "-d".into(),
        "/tmp/store".into(),
    ];
    let parsed = parse_cli_args(&args).unwrap();
    assert_eq!(parsed.custom_model.as_deref(), Some("custom-model"));
    assert_eq!(parsed.custom_dir, Some(PathBuf::from("/tmp/store")));
    match parsed.command {
        CliCommand::SessionRun { session_id, input } => {
            assert_eq!(session_id, "sess_12345");
            assert_eq!(input, "继续");
        }
        _ => panic!("unexpected command"),
    }

    // 6. s subcommands with -m
    let args = vec![
        "ragent".into(),
        "s".into(),
        "list".into(),
        "-m".into(),
        "gpt-4o".into(),
    ];
    let parsed = parse_cli_args(&args).unwrap();
    assert_eq!(parsed.custom_model.as_deref(), Some("gpt-4o"));
    assert!(matches!(parsed.command, CliCommand::SessionList));

    let args = vec![
        "ragent".into(),
        "s".into(),
        "view".into(),
        "sess_abc".into(),
        "--model=gpt-4o".into(),
    ];
    let parsed = parse_cli_args(&args).unwrap();
    assert_eq!(parsed.custom_model.as_deref(), Some("gpt-4o"));
    match parsed.command {
        CliCommand::SessionShow { session_id } => assert_eq!(session_id, "sess_abc"),
        _ => panic!("unexpected command"),
    }

    // 7. Error cases for -m / --model
    let args = vec!["ragent".into(), "测试".into(), "-m".into()];
    assert!(parse_cli_args(&args).is_err());

    let args = vec!["ragent".into(), "测试".into(), "--model".into()];
    assert!(parse_cli_args(&args).is_err());

    let args = vec!["ragent".into(), "测试".into(), "-m".into(), "".into()];
    assert!(parse_cli_args(&args).is_err());

    let args = vec!["ragent".into(), "测试".into(), "--model=".into()];
    assert!(parse_cli_args(&args).is_err());

    let args = vec!["ragent".into(), "测试".into(), "-m=".into()];
    assert!(parse_cli_args(&args).is_err());
}

#[tokio::test]
async fn cli_model_override_takes_precedence_over_global_and_project_config() {
    let temp_global = tempfile::tempdir().unwrap();
    let global_config = r#"
[model]
name = "global-model"
temperature = 0.7
max_output_tokens = 2048

[model.reasoning]
effort = "medium"
summary = "auto"
"#;
    std::fs::write(temp_global.path().join("config.toml"), global_config).unwrap();

    let temp_project = tempfile::tempdir().unwrap();
    let project_dir = temp_project.path().join(".ragent");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project_config = r#"
[model]
name = "project-model"
temperature = 0.2

[model.reasoning]
effort = "high"
"#;
    let project_config_file = project_dir.join("config.toml");
    std::fs::write(&project_config_file, project_config).unwrap();

    let manager = HookManager::load_with_project_config(temp_global.path(), &project_config_file)
        .await
        .unwrap();

    // 1. Without model override: fallback to project / global
    let mut config_default =
        AgentConfig::new("https://example.com", "fake_key", "initial-placeholder");
    let settings = manager.model_settings().unwrap();
    config_default.apply_model_settings(settings);
    assert_eq!(config_default.model, "project-model");
    assert_eq!(config_default.temperature, Some(0.2));
    assert_eq!(config_default.max_output_tokens, Some(2048));
    assert_eq!(
        config_default.reasoning.as_ref().unwrap().effort,
        Some(ReasoningEffort::High)
    );

    // 2. With CLI model override: overrides project/global model name, but keeps other settings
    let mut config_overridden =
        AgentConfig::new("https://example.com", "fake_key", "initial-placeholder")
            .with_model("cli-override-model");
    config_overridden.apply_model_settings(settings);
    assert_eq!(config_overridden.model, "cli-override-model");
    assert_eq!(config_overridden.temperature, Some(0.2));
    assert_eq!(config_overridden.max_output_tokens, Some(2048));
    assert_eq!(
        config_overridden.reasoning.as_ref().unwrap().effort,
        Some(ReasoningEffort::High)
    );
}
