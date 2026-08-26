[English](README.md) | **简体中文**

# ragent

`ragent` 是一个以 WebAssembly Component 扩展为核心的极简流式 LLM Agent。

本体只保留三类职责：

- 模型请求与流式响应 I/O
- 上下文提交
- Agent/ReAct 循环

工具、System Prompt 调整、模型参数调整、输入处理、上下文处理以及各阶段通知都通过 WASM 扩展完成。项目本身不硬编码任何内置工具；仓库中提供了可选的 Shell 与 File Editor 示例扩展。

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
- 构建扩展时需要 `wasm32-wasip2` target
- 一个兼容 OpenAI Responses API 的模型服务

安装 Rust WASM target：

```sh
rustup target add wasm32-wasip2
```

## 构建

构建 Agent：

```sh
cargo build --release
```

构建仓库提供的示例扩展（`shell` 和 `file_editor`）：

```sh
# 默认构建所有扩展
./scripts/build-extensions.sh

# 或指定构建部分扩展（逗号分隔）
./scripts/build-extensions.sh shell
./scripts/build-extensions.sh shell,file_editor
```

生成的 Component 位于：

```text
extensions/shell/target/wasm32-wasip2/release/ragent_shell_extension.wasm
extensions/file_editor/target/wasm32-wasip2/release/ragent_file_editor_extension.wasm
```

构建并安装或更新到当前 ragent 配置目录：

```sh
# 默认安装所有扩展
./scripts/install-extensions.sh

# 或指定安装部分扩展
./scripts/install-extensions.sh shell
./scripts/install-extensions.sh file_editor
```

安装/更新脚本优先使用 `XDG_CONFIG_HOME`，未设置时使用 `~/.config/ragent`。它会原子
替换安装的扩展（`extensions/shell.wasm`、`extensions/file_editor.wasm`）；若 `config.toml` 不存在，则创建默认配置；若 `config.toml` 已存在，则智能检查并在末尾追加缺失的扩展项，避免重复添加或破坏已有配置。

## 配置

模型连接通过环境变量提供：

```sh
export ROSETTA_URL="https://example.com/v1"
export ROSETTA_TOKEN="your-token"
export MODEL_NAME="your-model"
```

扩展通过 `~/.config/ragent/config.toml` 加载。首次安装时，安装脚本会创建以下配置：

```toml
[[extensions]]
name = "shell"
path = "extensions/shell.wasm"
enabled = true

[extensions.config]
default_timeout_seconds = 1800

[[extensions]]
name = "file_editor"
path = "extensions/file_editor.wasm"
enabled = true
```

配置文件当前只负责发现扩展和传递扩展初始化配置。相对路径以 `~/.config/ragent/` 为基准。
Shell 未配置时默认超时 1800 秒（30 分钟）；配置为 `0` 可禁用超时。每次
Shell 工具调用还可以通过可选参数 `timeout_seconds` 覆盖，单次传 `0` 同样表示禁用。

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

运行期间第一次按 `Ctrl+C` 会取消当前模型、Hook 或工具任务，随后执行扩展
shutdown 并保存会话。清理过程中再次按下会立即以状态码 130 退出。

## 默认行为

- 默认 System Prompt 只有：`你是一个高效、精准的 AI 智能体助手`
- 未加载扩展时工具列表为空
- 不进行自动上下文裁剪
- `max_iterations = 0` 表示不限制循环轮数
- Session ID 只允许字母、数字、`_`、`-`，最大 64 个字符
- 嵌入式调用方可以通过 `AgentSender::cancel()` 走同一条取消流程

## 验证

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

扩展测试覆盖：
- Shell: 成功命令、非零退出码、配置默认超时、单次覆盖和禁用超时。
- File Editor: 全量创建与写入 (`write_file`)、唯一性增量查找替换 (`replace_in_file`)、多处匹配拦截报错、错误路径处理。

## 项目结构

```text
src/agent.rs              Agent I/O 与 loop
src/context.rs            上下文保存与提交
src/wasm/                 WASM 加载、调度和协议类型
wit/ragent-extension.wit  Component ABI
extensions/               仓库提供的可选扩展示例 (shell, file_editor)
EXTENSIONS.zh-CN.md       扩展开发与 Hook 协议
```
