# ragent 扩展开发与 Hook 协议

本文档描述当前代码已经实现的 WASM Component 扩展协议。协议入口见 [`wit/ragent-extension.wit`](wit/ragent-extension.wit)，Rust 数据结构见 [`src/wasm/types.rs`](src/wasm/types.rs)，可运行的完整示例见 [`extensions/shell`](extensions/shell)。

## 1. 设计目标

Agent 本体只负责模型 I/O、上下文提交和循环控制。扩展可以在各阶段修改：

- System Prompt
- 模型名称、温度、最大输出 Token
- 工具列表、工具参数和工具结果
- 用户输入、模型请求和模型响应
- 待提交的上下文
- 是否继续下一轮

WASM 在这里是跨语言扩展载体，不是安全沙箱。扩展能否执行某项宿主能力由 WIT import 决定；当前宿主只提供 `host.execute-command`，它会直接在宿主机上通过 `sh -c` 执行命令。不要加载不受信任的 Shell 扩展。

## 2. Component ABI

所有扩展必须实现 `plugin` world：

```wit
package ragent:extension@1.0.0;

interface host {
    record command-output {
        exit-code: s32,
        stdout: string,
        stderr: string,
        error: option<string>,
    }

    execute-command: func(command: string) -> command-output;
}

interface lifecycle {
    metadata: func() -> string;
    initialize: func(config: string) -> result<_, string>;
    invoke: func(request: string) -> result<string, string>;
    shutdown: func();
}

world plugin {
    import host;
    export lifecycle;
}
```

四个生命周期方法的含义：

| 方法 | 调用次数 | 说明 |
| --- | ---: | --- |
| `metadata` | 加载时一次 | 返回扩展身份和 Hook 订阅的 JSON |
| `initialize` | Agent 初始化时一次 | 接收配置文件中 `[extensions.config]` 对应的 JSON |
| `invoke` | 每次 Hook | 接收 HookRequest JSON，返回 HookResult JSON或错误字符串 |
| `shutdown` | Agent 关闭时一次 | 释放扩展内部状态 |

选择单一 JSON `invoke` 而不是为每个 Hook 增加 WIT 方法，是为了在新增 Hook 时保持 Component ABI 稳定。JSON 协议当前为 `protocol_version = 1`。

## 3. 元数据

`metadata()` 返回：

```json
{
  "id": "my-extension",
  "version": "0.1.0",
  "subscriptions": [
    {
      "hook": "turn.prepare",
      "kind": "transform",
      "priority": 100,
      "failure": "abort"
    }
  ]
}
```

字段规则：

- `id` 是扩展的真实身份，必须非空且在一次加载中唯一。
- `version` 是扩展自己的版本，不是 Hook 协议版本。
- `hook` 必须使用本文列出的 Hook 名称。
- `kind` 只能是 `transform`、`action`、`observer`。
- `priority` 越小越先执行；相同优先级保持配置加载顺序。
- `failure` 为 `abort` 或 `ignore`。
- 同一扩展不能重复声明相同 Hook 和类型。

配置文件中的 `name` 只是加载时使用的名称，所有权和去重以元数据 `id` 为准。

## 4. 通用请求与返回

### 4.1 HookRequest

`invoke()` 收到的字符串反序列化后为：

```json
{
  "hook": "turn.prepare",
  "protocol_version": 1,
  "invocation_id": 42,
  "iteration": 2,
  "payload": {}
}
```

- `invocation_id` 是进程内单调递增的调用编号，可用于日志关联。
- `iteration` 只在轮次内 Hook 出现；初始化、输入和关闭 Hook 可能没有该字段。
- `payload` 的具体结构由 Hook 点决定。

扩展应拒绝自己不支持的 `protocol_version`，不要猜测结构。

### 4.2 HookResult

Transform 和 Action 返回统一信封：

```json
{
  "action": "continue",
  "payload": {},
  "reason": null
}
```

| action | Transform 含义 | Action 含义 |
| --- | --- | --- |
| `continue` | 用 `payload` 替换当前草稿并继续 | 返回 Action 结果；必须有 `payload` |
| `unchanged` | 保留输入并继续 | 不允许 |
| `reject` | 以 `reason` 拒绝并终止 | 以 `reason` 拒绝并终止 |
| `skip` | 跳过当前阶段，具体效果由 Hook 决定 | 不允许 |
| `stop` | 停止当前外围流程，具体效果由 Hook 决定 | 不允许 |

Observer 的返回值被忽略，但 `invoke()` 的成功分支仍必须返回合法 JSON 字符串，例如 `{"action":"unchanged"}`。

### 4.3 三种调度类型

#### Transform

按优先级串行执行，后一扩展接收前一扩展已经校验过的结果。它既能修改，也能从空列表中添加能力，因此不再单独设计 Provider。

每个返回都会立即校验：

- `failure = "abort"`：错误包含扩展 ID 和 Hook 名，Agent 终止。
- `failure = "ignore"`：丢弃本扩展结果，回滚到它执行前的值，然后继续。

`reject` 已覆盖 Gate 的用途，因此没有独立 Gate 类型。

#### Action

用于实际副作用。核心根据不可伪造的 Owner 将调用路由给唯一扩展，不做广播。当前 Action 只有 `tools.call`。

#### Observer

向所有订阅者并发广播。返回内容不参与流程；`abort` 策略下调用错误仍会传播，`ignore` 下忽略该扩展错误。

## 5. AgentDraft 与工具所有权

`agent.prepare` 和 `turn.prepare` 使用同一个 AgentDraft：

```json
{
  "system_prompt": "你是一个高效、精准的 AI 智能体助手",
  "model": {
    "name": "example-model",
    "temperature": null,
    "max_output_tokens": null
  },
  "tools": [],
  "context": null
}
```

`turn.prepare` 中的 `context` 为只读：

```json
{
  "items": ["Open Responses Item ..."],
  "view": {
    "items_count": 3,
    "recent_messages": [
      {"role": "user", "content": "hello"}
    ]
  }
}
```

扩展可以根据完整 `items` 或轻量 `view` 动态调整工具、Prompt 和模型参数。核心会把扩展返回的 `context` 恢复为输入值，扩展不能在这个 Hook 中提交上下文；需要修改上下文时使用 `context.commit`。

### 5.1 添加工具

新工具必须省略 `id` 和 `owner`：

```json
{
  "enabled": true,
  "name": "weather",
  "description": "查询天气",
  "parameters": {
    "type": "object",
    "properties": {
      "city": {"type": "string"}
    },
    "required": ["city"]
  }
}
```

核心返回给后续扩展时会变为：

```json
{
  "id": "tool-1",
  "owner": "weather-extension",
  "enabled": true,
  "name": "weather",
  "description": "查询天气",
  "parameters": {"type": "object"}
}
```

所有权规则：

- 新增工具的扩展必须同时以 `action` 订阅 `tools.call`。
- 新工具不能自行填写 ID；未知 ID 会被拒绝。
- 修改已有工具时 ID 必须保留，Owner 会由核心恢复，不能夺取所有权。
- 工具可以修改、禁用、删除或重排。
- 所有工具名称和 ID 都必须唯一，即使工具被禁用也不能重名。
- `parameters` 顶层 `type` 当前必须为 `object`。

### 5.2 模型参数校验

- `system_prompt` 不能为空。
- 模型名称不能为空。
- `temperature` 必须为空或位于 `0..=2`。
- `max_output_tokens` 必须为空或大于 0。

## 6. Agent 流程

```text
读取 ~/.config/ragent/config.toml
  -> 加载 Component / metadata
  -> 校验订阅
  -> initialize
  -> agent.prepare
  -> 保存 BaseAgentState

输入
  -> input.prepare
  -> context.commit(reason=input)

每一轮
  -> clone BaseAgentState + 当前完整上下文
  -> turn.prepare
  -> model.request.prepare
  -> 模型流式 I/O
       -> model.stream.observe（每个标准化事件）
  -> model.response.prepare
  -> context.commit(reason=model_response)
  -> 若包含工具调用：
       tool.call.prepare
       -> tools.call（路由给工具 Owner）
       -> tool.result.prepare
       -> context.commit(reason=tool_results)
  -> turn.complete
  -> 继续或结束

运行错误 -> agent.error
关闭     -> agent.shutdown -> lifecycle.shutdown
```

`BaseAgentState` 不会被每轮动态变更永久污染。若要永久添加工具或修改默认 Prompt，在 `agent.prepare` 修改；若要按上下文动态变化，在 `turn.prepare` 修改。

## 7. Hook 点位与载荷

下表中的“返回”指 `HookResult.payload`。`unchanged` 不需要携带 payload。

| Hook | 类型 | 时机 | 输入/返回 payload | skip / stop |
| --- | --- | --- | --- | --- |
| `agent.prepare` | Transform | 初始化扩展后 | `AgentDraft -> AgentDraft` | 两者都会使 Agent 初始化失败 |
| `input.prepare` | Transform | 用户输入提交前 | `{text:string, delayed:bool}` | skip 丢弃输入；stop 停止运行 |
| `turn.prepare` | Transform | 每轮构造请求前 | `AgentDraft -> AgentDraft`，含只读 context | skip 跳过本轮；stop 结束 loop |
| `model.request.prepare` | Transform | 调用模型前 | Open Responses `CreateResponseBody` JSON | skip 跳过本轮；stop 结束 loop |
| `model.stream.observe` | Observer | 每个流事件 | 标准化事件 JSON | 不适用 |
| `model.response.prepare` | Transform | 流结束、提交前 | `{text:string, items:Item[]}` | skip 丢弃响应并进入下一轮；stop 结束 loop |
| `tool.call.prepare` | Transform | Action 前 | `ToolCallRequest -> ToolCallRequest` | skip 生成“已跳过”工具输出；stop 结束 loop |
| `tools.call` | Action | 实际执行工具 | `ToolCallRequest -> ToolResult` | Action 不允许 skip/stop |
| `tool.result.prepare` | Transform | 工具结果提交前 | `{call:ToolCallRequest,result:ToolResult}` | 当前 skip/stop 均保留进入 Hook 前的结果 |
| `context.commit` | Transform | 任意上下文写入前 | `ContextCommitDraft -> ContextCommitDraft` | 不提交；调用方停止或跳过对应阶段 |
| `turn.complete` | Transform | 一轮结束时 | `{iteration,called_tools,continue_loop}` | skip 保持原决定；stop 结束 loop |
| `agent.error` | Observer | `run()` 返回错误时 | `{error:string}` | 不适用；Observer 错误在此处不会覆盖原错误 |
| `agent.shutdown` | Observer | 生命周期 shutdown 前 | `{}` | 不适用；即使 Observer 报错仍会调用 lifecycle shutdown |

### 7.1 model.stream.observe

标准化事件目前有：

```json
{"type":"text_delta","delta":"partial text"}
{"type":"output_item_added","item":{}}
{"type":"output_item_done","item":{}}
{"type":"error","message":"..."}
{"type":"other"}
```

这是高频 Hook。不要在里面执行昂贵的同步工作。

### 7.2 ToolCallRequest / ToolResult

```json
{
  "call_id": "call_123",
  "tool_id": "tool-1",
  "name": "shell",
  "arguments": {"command":"pwd"}
}
```

`tool.call.prepare` 可以修改 `arguments`。`call_id` 和 `tool_id` 不可修改；`name` 最终会由核心恢复为该 ToolEntry 的名称。

Action 必须返回：

```json
{
  "action": "continue",
  "payload": {
    "success": true,
    "output": "result text",
    "error": null
  }
}
```

工具失败也应返回正常的 HookResult，并令 `success = false`；只有扩展协议本身失败时才返回 WIT `err(string)`。

### 7.3 context.commit

```json
{
  "reason": "model_response",
  "current": [],
  "pending": [],
  "next": []
}
```

- `reason` 为 `input`、`model_response` 或 `tool_results`。
- `current` 是提交前上下文。
- `pending` 是本次准备追加的 Item。
- `next` 默认是 `current + pending`，扩展应修改并返回 `next`。

核心会反序列化 `next` 并检查：System Message 不能混入 Items、Function Call ID 不能重复、Function Call Output 必须引用已经出现的 Call ID。System Prompt 始终通过模型请求的 `instructions` 单独提交。

## 8. 用 Rust 开发

Rust 是当前仓库完整验证的开发路径。最简单的方式是复制 [`extensions/shell`](extensions/shell)。

`Cargo.toml`：

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
wit-bindgen = "0.46"
```

入口骨架：

```rust
wit_bindgen::generate!({
    path: "../../wit",
    world: "plugin",
});

struct Extension;

impl exports::ragent::extension::lifecycle::Guest for Extension {
    fn metadata() -> String {
        serde_json::json!({
            "id": "example",
            "version": "0.1.0",
            "subscriptions": [{
                "hook": "turn.prepare",
                "kind": "transform",
                "priority": 100,
                "failure": "abort"
            }]
        }).to_string()
    }

    fn initialize(_config: String) -> Result<(), String> { Ok(()) }

    fn invoke(request: String) -> Result<String, String> {
        let request: serde_json::Value =
            serde_json::from_str(&request).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "action": "continue",
            "payload": request["payload"].clone()
        }).to_string())
    }

    fn shutdown() {}
}

export!(Extension);
```

构建 core Wasm 并转换为 Component：

```sh
cargo build --target wasm32-unknown-unknown --release
cargo run --manifest-path /path/to/ragent/Cargo.toml \
  --example componentize -- \
  target/wasm32-unknown-unknown/release/example.wasm \
  example.component.wasm
```

仓库脚本 [`scripts/build-extensions.sh`](scripts/build-extensions.sh) 就是这条流程。

## 9. 用 Go 开发

推荐使用 Bytecode Alliance 的 [`componentize-go`](https://github.com/bytecodealliance/componentize-go) 或 `wit-bindgen go`。当前工具要求和生成 API仍在变化，应先执行本机版本的 `--help`，并以生成代码中的函数签名为准。

基本流程：

```sh
go mod init example.com/ragent-extension
go install github.com/bytecodealliance/componentize-go@latest
componentize-go --help
```

将本项目 `wit/` 复制或链接到 Go 模块，选择 world `plugin`，生成绑定；然后在生成的 lifecycle export 包中实现 `Metadata`、`Initialize`、`Invoke`、`Shutdown`。所有业务结构仍是本文定义的 JSON，不需要在 Go 中重新实现 Hook 调度协议。

也可以使用较底层的流程：

```sh
wit-bindgen go -w plugin /path/to/ragent/wit
go mod tidy
GOARCH=wasm GOOS=wasip1 go build \
  -buildmode=c-shared -ldflags=-checklinkname=0 -o core.wasm
wasm-tools component embed -w plugin /path/to/ragent/wit \
  core.wasm -o core-with-wit.wasm
wasm-tools component new core-with-wit.wasm -o extension.wasm \
  --adapt wasi_snapshot_preview1.reactor.wasm
```

注意：ragent 当前只链接 `ragent:extension/host`，没有链接通用 WASI 接口。Go 产物如果还导入 `wasi:*`，会在加载时失败。交付前必须用下文的 `wasm-tools component wit` 检查；在宿主加入 WASI 支持前，Go 路线属于需要自行处理 WASI 依赖的实验性路线。

## 10. 用 AssemblyScript 开发

AssemblyScript 能生成 core Wasm，但当前没有本项目可直接采用的一等 WIT Component 绑定生成器。仅用 `asc` 得到的 `.wasm` 不是 Component，不能直接放进配置。

实验性路线需要：

1. 用 AssemblyScript 实现 lifecycle 和 host import。
2. 自行实现 WIT Canonical ABI 的字符串、record 和 result lowering/lifting。
3. 为 core Wasm 嵌入 `plugin` world 类型信息。
4. 用 `wasm-tools component new` 包装为 Component。

```sh
npx asc assembly/index.ts -o core.wasm
wasm-tools component embed -w plugin /path/to/ragent/wit \
  core.wasm -o core-with-wit.wasm
wasm-tools component new core-with-wit.wasm -o extension.wasm
```

上面的命令不会替你生成 Canonical ABI；如果第 2 步没有正确完成，Component 会在转换或实例化时失败。因此当前更实际的 TypeScript 路线是编译为 ESM JavaScript，再使用 Bytecode Alliance 的 [`ComponentizeJS`](https://github.com/bytecodealliance/ComponentizeJS)。这不是 AssemblyScript 二进制路线，但可以直接从 TypeScript/JavaScript 逻辑生成 Component。

由于 ragent 不提供 WASI imports，ComponentizeJS 应通过其 Node API关闭 `stdio`、`random`、`clocks`、`http`、`fetch-event`，生成只依赖目标 WIT world 的 pure component。

## 11. 用 Python 开发

使用 Bytecode Alliance 的 [`componentize-py`](https://github.com/bytecodealliance/componentize-py)，需要 Python 3.10 或更高版本：

```sh
python3 -m venv .venv
. .venv/bin/activate
pip install componentize-py
componentize-py -d /path/to/ragent/wit -w plugin \
  bindings ragent_guest
```

生成绑定后，按照生成包中的 lifecycle 基类实现四个方法。Python 中仍然只需要：

- `metadata()` 返回 JSON 字符串。
- `initialize(config)` 解析 JSON 配置。
- `invoke(request)` 解析 HookRequest 并返回 HookResult JSON 字符串。
- `shutdown()` 释放状态。

构建：

```sh
componentize-py -d /path/to/ragent/wit -w plugin \
  componentize --stub-wasi app -o extension.wasm
```

必须使用 `--stub-wasi` 或以其他方式确保最终 Component 不依赖宿主未提供的 WASI imports。Python 会把解释器和应用一起打进 Component，因此产物通常明显大于 Rust 扩展，加载时间也更长。

## 12. 检查 Component

无论使用哪种语言，最终文件必须是 Component，不是普通 core Wasm：

```sh
wasm-tools validate extension.wasm --features component-model
wasm-tools component wit extension.wasm
```

提取出的 world 必须兼容 `ragent:extension/plugin@1.0.0`。当前宿主只允许项目 WIT 中的 `ragent:extension/host` import；如果输出还列出 `wasi:*` 或其他 import，ragent 目前无法实例化它。

然后用临时配置做加载测试：

```toml
[[extensions]]
name = "example"
path = "/absolute/path/to/extension.wasm"
enabled = true
```

## 13. 安装与加载扩展

默认目录结构：

```text
~/.config/ragent/
├── config.toml
└── extensions/
    ├── shell.wasm
    └── example.wasm
```

`config.toml`：

```toml
[[extensions]]
name = "example"
path = "extensions/example.wasm"
enabled = true

[extensions.config]
endpoint = "https://example.com"
mode = "strict"
```

加载规则：

- 默认配置目录是 `~/.config/ragent/`。
- 若设置 `XDG_CONFIG_HOME`，则使用 `$XDG_CONFIG_HOME/ragent/`。
- 相对 Component 路径相对于配置目录解析。
- 配置不存在时，Agent 会创建配置目录，但不会自动启用任何扩展。
- `enabled = false` 的条目不会加载。
- 扩展按配置顺序加载和初始化。
- 任一启用扩展文件不存在、元数据无效或初始化失败，Agent 初始化失败。
- 配置修改后需要重新启动 Agent；当前没有热重载。

多个扩展示例：

```toml
[[extensions]]
name = "base-tools"
path = "extensions/base-tools.wasm"
enabled = true

[[extensions]]
name = "policy"
path = "extensions/policy.wasm"
enabled = true

[extensions.config]
deny_shell = true
```

如果两个扩展订阅同一个 Transform，先按 `priority`，再按上述配置顺序执行。

## 14. 调试建议

- 先让 `metadata` 只订阅一个 Hook，确认能加载后再扩展。
- `failure = "abort"` 适合开发期，可保留完整错误；可选增强功能上线后再考虑 `ignore`。
- 在日志中记录 `invocation_id`、`hook`、`iteration`，不要记录 API Key 或敏感上下文。
- Transform 应尽量保持纯函数；副作用放到 Action 或 Observer。
- `model.stream.observe` 是高频路径，避免阻塞。
- 工具扩展必须测试成功、参数错误、宿主错误、非零退出码四类结果。
- 不要依赖另一个扩展生成的临时工具 ID；ID 只保证当前 Agent 进程和当前草稿链路内稳定。
- 若 Component 能通过 `wasm-tools validate` 但无法加载，首先检查它是否含有宿主未链接的额外 imports。
