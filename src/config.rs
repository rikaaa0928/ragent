use openresponses_rust::{ReasoningConfig, ReasoningEffort, ReasoningSummary};
use serde::{Deserialize, Serialize};

/// 控制是否将思考摘要（Reasoning Summary）放入发送给大模型的上下文历史中
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContextSummaryMode {
    /// 开启：始终将思考摘要包含在发送给模型的上下文历史中
    On,
    /// 关闭（默认）：始终从发送给模型的上下文历史中清空思考摘要文本（节省上下文 Token，同时保留签名载体）
    #[default]
    Off,
}

impl std::fmt::Display for ContextSummaryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::On => write!(f, "on"),
            Self::Off => write!(f, "off"),
        }
    }
}

/// 配置文件中的 `[model.reasoning]` 选项块
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModelReasoningSettings {
    /// 思考强度 (none, minimal, low, medium, high, xhigh)
    #[serde(default)]
    pub effort: Option<ReasoningEffort>,
    /// 思考摘要策略 (auto, concise, detailed)
    #[serde(default)]
    pub summary: Option<ReasoningSummary>,
    /// 是否将思考摘要放入上下文 (on, off，默认 off)
    #[serde(default)]
    pub context_summary: Option<ContextSummaryMode>,
}

impl From<ReasoningConfig> for ModelReasoningSettings {
    fn from(c: ReasoningConfig) -> Self {
        Self {
            effort: c.effort,
            summary: c.summary,
            context_summary: None,
        }
    }
}

/// 配置文件中的 `[model]` 选项块
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ModelSettings {
    /// 模型名称 (可选)
    #[serde(default)]
    pub name: Option<String>,
    /// 生成温度 (0.0 - 2.0, 可选)
    #[serde(default)]
    pub temperature: Option<f64>,
    /// 最大输出 Token 数 (可选)
    #[serde(default)]
    pub max_output_tokens: Option<i32>,
    /// 思考/推理配置 (可选，包含 effort, summary 与 context_summary)
    #[serde(default)]
    pub reasoning: Option<ModelReasoningSettings>,
}

/// Agent 运行与连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// 目标 API 的 Base URL
    pub base_url: String,
    /// 授权 Token / API Key
    pub api_key: String,
    /// 模型名称 (如 gemini-3.7-flash, gpt-4o 等)
    pub model: String,
    /// 显式覆盖的模型名称 (若存在，则 apply_model_settings 不会覆盖 model 字段)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    /// 生成温度 (0.0 - 2.0, 可选)
    pub temperature: Option<f64>,
    /// 最大输出 Token 数 (可选)
    pub max_output_tokens: Option<i32>,
    /// 思考/推理配置 (默认 None)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    /// 上下文思考摘要注入策略 (默认 Off)
    #[serde(default)]
    pub context_summary: ContextSummaryMode,
}

impl AgentConfig {
    /// 创建基础配置
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            model_override: None,
            temperature: None,
            max_output_tokens: None,
            reasoning: None,
            context_summary: ContextSummaryMode::default(),
        }
    }

    /// 应用来自 `config.toml` 中 `[model]` 块的配置
    pub fn apply_model_settings(&mut self, settings: &ModelSettings) {
        if self.model_override.is_none() {
            if let Some(ref name) = settings.name {
                self.model = name.clone();
            }
        }
        if settings.temperature.is_some() {
            self.temperature = settings.temperature;
        }
        if settings.max_output_tokens.is_some() {
            self.max_output_tokens = settings.max_output_tokens;
        }
        if let Some(ref reasoning) = settings.reasoning {
            if reasoning.effort.is_some() || reasoning.summary.is_some() {
                self.reasoning = Some(ReasoningConfig {
                    effort: reasoning.effort.clone(),
                    summary: reasoning.summary.clone(),
                });
            }
            if let Some(mode) = reasoning.context_summary {
                self.context_summary = mode;
            }
        }
    }

    /// 设置并覆盖模型名称 (优先级高于配置文件中的 [model] name)
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let m = model.into();
        self.model = m.clone();
        self.model_override = Some(m);
        self
    }

    /// 设置显式模型覆盖 (若为 None 则恢复由配置文件填充)
    pub fn with_model_override(mut self, model: Option<String>) -> Self {
        if let Some(ref m) = model {
            self.model = m.clone();
        }
        self.model_override = model;
        self
    }

    /// 从环境变量读取配置 (仅支持 ROSETTA_URL, ROSETTA_TOKEN，模型需由配置文件 [model] 块或后续配置提供)
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let base_url = std::env::var("ROSETTA_URL")?;
        let api_key = std::env::var("ROSETTA_TOKEN")?;
        Ok(Self::new(base_url, api_key, ""))
    }

    /// 设置温度参数
    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// 设置最大输出 Token
    pub fn with_max_output_tokens(mut self, max_tokens: i32) -> Self {
        self.max_output_tokens = Some(max_tokens);
        self
    }

    /// 设置推理/思考配置
    pub fn with_reasoning(mut self, reasoning: ReasoningConfig) -> Self {
        self.reasoning = Some(reasoning);
        self
    }

    /// 设置思考强度 (effort: none, low, medium, high, xhigh)
    pub fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        let mut r = self.reasoning.unwrap_or_default();
        r.effort = Some(effort);
        self.reasoning = Some(r);
        self
    }

    /// 设置思考总结策略 (summary: auto, concise, detailed)
    pub fn with_reasoning_summary(mut self, summary: ReasoningSummary) -> Self {
        let mut r = self.reasoning.unwrap_or_default();
        r.summary = Some(summary);
        self.reasoning = Some(r);
        self
    }

    /// 设置上下文思考摘要注入策略 (on, off)
    pub fn with_context_summary(mut self, mode: ContextSummaryMode) -> Self {
        self.context_summary = mode;
        self
    }
}
