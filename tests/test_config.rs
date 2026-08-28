mod common;

use common::{file_editor_component_path, shell_component_path};
use openresponses_rust::{ReasoningConfig, ReasoningEffort, ReasoningSummary};
use ragent::{AgentConfig, ExtensionManager};

#[tokio::test]
async fn config_loads_model_settings_and_reasoning_from_config_toml() {
    let temp = tempfile::tempdir().unwrap();
    let config_toml = r#"
[model]
name = "gemini-2.5-pro"
temperature = 0.4
max_output_tokens = 4096

[model.reasoning]
effort = "high"
summary = "concise"

[[extensions]]
name = "shell"
path = "extensions/shell.wasm"
enabled = false
"#;
    std::fs::write(temp.path().join("config.toml"), config_toml).unwrap();

    let manager = ExtensionManager::load_from_dir(temp.path()).await.unwrap();
    assert!(manager.model_settings().is_some());

    let settings = manager.model_settings().unwrap();
    assert_eq!(settings.name.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(settings.temperature, Some(0.4));
    assert_eq!(settings.max_output_tokens, Some(4096));

    let reasoning = settings.reasoning.as_ref().unwrap();
    assert_eq!(reasoning.effort, Some(ReasoningEffort::High));
    assert_eq!(reasoning.summary, Some(ReasoningSummary::Concise));

    let mut config = AgentConfig::new("https://example.com", "fake_key", "default-model");
    config.apply_model_settings(settings);

    assert_eq!(config.model, "gemini-2.5-pro");
    assert_eq!(config.temperature, Some(0.4));
    assert_eq!(config.max_output_tokens, Some(4096));
    assert_eq!(
        config.reasoning,
        Some(ReasoningConfig {
            effort: Some(ReasoningEffort::High),
            summary: Some(ReasoningSummary::Concise),
        })
    );
    assert_eq!(config.context_summary, ragent::ContextSummaryMode::Off);
}

#[tokio::test]
async fn config_loads_context_summary_modes_from_config_toml() {
    let temp = tempfile::tempdir().unwrap();
    let config_toml = r#"
[model]
name = "gemini-3.7-flash"

[model.reasoning]
effort = "high"
summary = "detailed"
context_summary = "off"
"#;
    std::fs::write(temp.path().join("config.toml"), config_toml).unwrap();

    let manager = ExtensionManager::load_from_dir(temp.path()).await.unwrap();
    let settings = manager.model_settings().unwrap();

    let mut config = AgentConfig::new("https://example.com", "fake_key", "default-model");
    config.apply_model_settings(settings);

    assert_eq!(
        config.reasoning,
        Some(ReasoningConfig {
            effort: Some(ReasoningEffort::High),
            summary: Some(ReasoningSummary::Detailed),
        })
    );
    assert_eq!(config.context_summary, ragent::ContextSummaryMode::Off);

    // Test on
    let config_on = r#"
[model.reasoning]
context_summary = "on"
"#;
    std::fs::write(temp.path().join("config.toml"), config_on).unwrap();
    let manager_on = ExtensionManager::load_from_dir(temp.path()).await.unwrap();
    let mut cfg_on = AgentConfig::new("https://example.com", "fake_key", "default-model");
    cfg_on.apply_model_settings(manager_on.model_settings().unwrap());
    assert_eq!(cfg_on.context_summary, ragent::ContextSummaryMode::On);
}

#[tokio::test]
async fn project_config_overrides_global_config_leaf_nodes() {
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

    let manager =
        ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file)
            .await
            .unwrap();

    let settings = manager.model_settings().expect("settings should exist");
    // name: overridden by project
    assert_eq!(settings.name.as_deref(), Some("project-model"));
    // temperature: overridden by project
    assert_eq!(settings.temperature, Some(0.2));
    // max_output_tokens: fallback to global
    assert_eq!(settings.max_output_tokens, Some(2048));

    let reasoning = settings.reasoning.as_ref().expect("reasoning should exist");
    // effort: overridden by project
    assert_eq!(reasoning.effort, Some(ReasoningEffort::High));
    // summary: fallback to global
    assert_eq!(reasoning.summary, Some(ReasoningSummary::Auto));
}

#[tokio::test]
async fn project_config_extension_restrictions_and_overrides() {
    let temp_global = tempfile::tempdir().unwrap();
    let global_config = format!(
        r#"
[[extensions]]
name = "shell"
path = {:?}
enabled = true
[extensions.config]
key1 = "val1"
nested = {{ a = 1, b = 2 }}

[[extensions]]
name = "file_editor"
path = {:?}
enabled = true
"#,
        shell_component_path().to_string_lossy(),
        file_editor_component_path().to_string_lossy()
    );
    std::fs::write(temp_global.path().join("config.toml"), global_config).unwrap();

    // 1. Valid override: modifies enabled and config on shell, disables file_editor
    let temp_project = tempfile::tempdir().unwrap();
    let project_dir = temp_project.path().join(".ragent");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project_config_file = project_dir.join("config.toml");

    let project_config_valid = r#"
[[extensions]]
name = "file_editor"
enabled = false

[[extensions]]
name = "shell"
[extensions.config]
key2 = "val2"
nested = { b = 3, c = 4 }
"#;
    std::fs::write(&project_config_file, project_config_valid).unwrap();

    let manager =
        ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file)
            .await
            .unwrap();

    // Only shell is enabled, file_editor disabled
    assert_eq!(manager.plugins().len(), 1);
    assert_eq!(manager.plugins()[0].metadata().id, "shell");

    // 2. Reject non-existent extension name in project config
    let project_config_unknown_name = r#"
[[extensions]]
name = "unknown_ext"
enabled = true
"#;
    std::fs::write(&project_config_file, project_config_unknown_name).unwrap();
    let err =
        ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file).await;
    let err_msg = match err {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected error"),
    };
    assert!(err_msg.contains("does not exist in global config"));

    // 3. Reject fields other than name, enabled, config in project config
    let project_config_invalid_fields = r#"
[[extensions]]
name = "shell"
path = "some/other/path.wasm"
"#;
    std::fs::write(&project_config_file, project_config_invalid_fields).unwrap();
    let err =
        ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file).await;
    assert!(err.is_err());

    // 4. Reject duplicate extension name in project config
    let project_config_duplicate = r#"
[[extensions]]
name = "shell"
enabled = true

[[extensions]]
name = "shell"
enabled = false
"#;
    std::fs::write(&project_config_file, project_config_duplicate).unwrap();
    let err =
        ExtensionManager::load_with_project_config(temp_global.path(), &project_config_file).await;
    let err_msg = match err {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected error"),
    };
    assert!(err_msg.contains("duplicate extension name"));

    // 5. Reject duplicate extension name in global config
    let temp_global_dup = tempfile::tempdir().unwrap();
    let global_config_dup = format!(
        r#"
[[extensions]]
name = "shell"
path = {:?}

[[extensions]]
name = "shell"
path = {:?}
"#,
        shell_component_path().to_string_lossy(),
        shell_component_path().to_string_lossy()
    );
    std::fs::write(
        temp_global_dup.path().join("config.toml"),
        global_config_dup,
    )
    .unwrap();
    let err =
        ExtensionManager::load_with_project_config(temp_global_dup.path(), &project_config_file)
            .await;
    let err_msg = match err {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected error"),
    };
    assert!(err_msg.contains("duplicate extension name"));
}
