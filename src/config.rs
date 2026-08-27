use openresponses_rust::{ReasoningConfig, ReasoningEffort, ReasoningSummary};
use serde::{Deserialize, Serialize};

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
    /// 思考/推理配置 (可选，包含 effort 与 summary)
    #[serde(default)]
    pub reasoning: Option<ReasoningConfig>,
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
    /// 生成温度 (0.0 - 2.0, 可选)
    pub temperature: Option<f64>,
    /// 最大输出 Token 数 (可选)
    pub max_output_tokens: Option<i32>,
    /// 最大保护循环轮次 (默认 0，0 表示不限制)
    pub max_iterations: usize,
    /// 思考/推理配置 (默认 None)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
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
            temperature: None,
            max_output_tokens: None,
            max_iterations: 0,
            reasoning: None,
        }
    }

    /// 应用来自 `config.toml` 中 `[model]` 块的配置
    pub fn apply_model_settings(&mut self, settings: &ModelSettings) {
        if let Some(ref name) = settings.name {
            self.model = name.clone();
        }
        if settings.temperature.is_some() {
            self.temperature = settings.temperature;
        }
        if settings.max_output_tokens.is_some() {
            self.max_output_tokens = settings.max_output_tokens;
        }
        if settings.reasoning.is_some() {
            self.reasoning = settings.reasoning.clone();
        }
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

    /// 设置最大循环次数保护
    pub fn with_max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
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
}
