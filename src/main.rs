use ragent::cli::run_cli;
use ragent::AgentConfig;
use std::env;
use std::process;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    // 尝试从环境变量加载配置；若缺失则提供占位配置（离线命令如 list/view/del 可正常使用）
    let config = AgentConfig::from_env().unwrap_or_else(|_| {
        AgentConfig::new(
            "https://placeholder-url",
            "placeholder-token",
            "gemini-3.7-flash",
        )
    });

    if let Err(err) = run_cli(&args, config).await {
        eprintln!("\n[执行错误]: {}", err);
        process::exit(1);
    }
}
