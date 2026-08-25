[English](README.md) | **简体中文**

# ragent

`ragent` 是一个以 WebAssembly Component 扩展为核心的极简流式 LLM Agent。

本体只保留三类职责：

- 模型请求与流式响应 I/O
- 上下文提交
- Agent/ReAct 循环

工具、System Prompt 调整、模型参数调整、输入处理、上下文处理以及各阶段通知都通过 WASM 扩展完成。项目本身不硬编码任何内置工具；仓库中的 Shell 仅是一个可选示例扩展。

## 工作方式

```text
用户输入
  -> WASM hooks
  -> 构造模型请求
  -> 流式模型 I/O
  -> WASM hooks
  -> 可选工具 Action
  -> 上下文提交
  -> 下一轮或结束
```

扩展使用统一的 WIT Component ABI 和 JSON Hook 协议。完整的架构、Hook 点位、请求与返回结构、工具所有权规则，以及 Rust、Go、AssemblyScript、Python 开发方式见：

> [扩展开发与 Hook 协议](EXTENSIONS.zh-CN.md)

## 环境要求

- Rust 工具链
- 构建示例扩展时需要 `wasm32-unknown-unknown` target
- 一个兼容 OpenAI Responses API 的模型服务

安装 Rust WASM target：

```sh
rustup target add wasm32-unknown-unknown
```

## 构建

构建 Agent：

```sh
cargo build --release
```

构建仓库提供的 Shell 示例扩展：

```sh
./scripts/build-extensions.sh
```

生成的 Component 位于：

```text
extensions/shell/target/component/ragent_shell_extension.wasm
```

## 配置

模型连接通过环境变量提供：

```sh
export ROSETTA_URL="https://example.com/v1"
export ROSETTA_TOKEN="your-token"
export MODEL_NAME="your-model"
```

扩展通过 `~/.config/ragent/config.toml` 加载。安装 Shell 示例扩展：

```sh
mkdir -p ~/.config/ragent/extensions
cp extensions/shell/target/component/ragent_shell_extension.wasm \
  ~/.config/ragent/extensions/shell.wasm
```

然后创建配置：

```toml
[[extensions]]
name = "shell"
path = "extensions/shell.wasm"
enabled = true
```

配置文件当前只负责发现扩展和传递扩展初始化配置。相对路径以 `~/.config/ragent/` 为基准。

## 使用

直接执行一次会话：

```sh
cargo run -- "查看当前目录下有哪些文件"
```

指定会话存储目录：

```sh
cargo run -- "分析这个项目" -d .ragent/sessions
```

会话管理：

```sh
cargo run -- s list
cargo run -- s view sess_example
cargo run -- s sess_example "继续分析"
cargo run -- s del sess_example
```

查看完整命令：

```sh
cargo run -- --help
```

## 默认行为

- 默认 System Prompt 只有：`你是一个高效、精准的 AI 智能体助手`
- 未加载扩展时工具列表为空
- 不进行自动上下文裁剪
- `max_iterations = 0` 表示不限制循环轮数
- Session ID 只允许字母、数字、`_`、`-`，最大 64 个字符

## 验证

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Shell 扩展测试同时验证成功命令和非零退出码；非零退出码会作为失败结果返回给 Agent。

## 项目结构

```text
src/agent.rs              Agent I/O 与 loop
src/context.rs            上下文保存与提交
src/wasm/                 WASM 加载、调度和协议类型
wit/ragent-extension.wit  Component ABI
extensions/               仓库提供的可选扩展示例
EXTENSIONS.md             扩展开发与 Hook 协议
```
