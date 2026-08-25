use serde::{Deserialize, Serialize};

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
        }
    }

    /// 从环境变量读取配置 (ROSETTA_URL, ROSETTA_TOKEN, 默认 MODEL_NAME 为 gemini-3.7-flash)
    pub fn from_env() -> Result<Self, std::env::VarError> {
        let base_url = std::env::var("ROSETTA_URL")?;
        let api_key = std::env::var("ROSETTA_TOKEN")?;
        let model = std::env::var("MODEL_NAME").unwrap_or_else(|_| "gemini-3.7-flash".to_string());
        Ok(Self::new(base_url, api_key, model))
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
}
