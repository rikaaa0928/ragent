[English](EXTENSIONS.md) | **简体中文**

# ragent 扩展开发与 Hook 协议

本文档描述当前代码已经实现的 WASM Component 扩展协议。协议入口见 [`wit/ragent-extension.wit`](wit/ragent-extension.wit)，Rust 数据结构见 [`src/wasm/types.rs`](src/wasm/types.rs)，可运行的完整示例见 [`extensions/shell`](extensions/shell)、[`extensions/file_editor`](extensions/file_editor) 和 [`extensions/image_viewer`](extensions/image_viewer)。

## 1. 设计目标

Agent 本体只负责模型 I/O、上下文提交和循环控制。扩展可以在各阶段修改：

- System Prompt
- 模型名称、温度、最大输出 Token
- 工具列表、工具参数和工具结果
- 用户输入、模型请求和模型响应
- 待提交的上下文
- 是否继续下一轮

WASM 在这里是跨语言扩展载体，不是安全沙箱。扩展能否执行某项宿主能力由 WIT import 决定；项目 host 接口可以通过 `sh -c` 执行命令，运行时还会链接已支持的 WASI 0.2 接口。不要加载不受信任的扩展。

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

    execute-command-with-timeout: func(command: string, timeout-ms: u64) -> command-output;
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
| `shutdown` | Agent 关闭时一次 | 释放扩展内部状态，包括正常取消后的清理 |

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

### 4.4 JSON 类型约定

下文使用 TypeScript 语法描述线上的 JSON，而不是要求扩展使用 TypeScript：

- `field?: T` 表示字段可以省略。
- `T | null` 表示字段存在时允许为 JSON `null`。
- `JsonValue` 表示任意合法 JSON 值。
- 所有整数都必须在目标字段注明的范围内；`invocation_id` 在 JavaScript 中应按安全整数处理。
- 未注明可省略的字段必须存在。扩展不应依赖未知字段被保留，除非类型明确包含 `[key: string]`。

```ts
type JsonPrimitive = null | boolean | number | string;
type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };

type HookKind = "transform" | "action" | "observer";
type HookFailurePolicy = "abort" | "ignore";
type HookAction = "continue" | "unchanged" | "reject" | "skip" | "stop";

interface HookSubscription {
  hook: string;
  kind: HookKind;
  priority?: number;             // 省略时为 0，必须是有符号 32 位整数
  failure?: HookFailurePolicy;   // 省略时为 "abort"
}

interface ExtensionMetadata {
  id: string;
  version: string;
  subscriptions?: HookSubscription[]; // 省略时为空数组
}

interface ExtensionConfigItem {
  name: string;
  path: string;
  enabled?: boolean;             // 省略时为 true
  config?: JsonValue;            // 省略时为 null；传给 initialize
}

interface ExtensionsConfig {
  extensions?: ExtensionConfigItem[]; // 省略时为空数组
}

interface HookRequest<P = JsonValue> {
  hook: string;
  protocol_version: 1;
  invocation_id: number;         // 宿主 u64；跨语言日志关联标识
  iteration?: number;            // 轮次 Hook 才存在
  payload: P;
}

interface HookResult<P = JsonValue> {
  action: HookAction;
  payload?: P;                   // continue 必须有；unchanged/reject/skip/stop 可省略
  reason?: string;               // reject 建议提供
}
```

`metadata()` 返回 `ExtensionMetadata` 的 JSON 字符串；`initialize()` 接收任意 `JsonValue` 的 JSON 字符串；`invoke()` 接收 `HookRequest` 字符串并在 WIT `ok` 中返回 `HookResult` 字符串。

### 4.5 ragent 核心数据结构

```ts
interface ModelDraft {
  name: string;
  temperature?: number | null;       // 核心输出时存在；null 或 0..=2
  max_output_tokens?: number | null; // 核心输出时存在；null 或正的 i32
}

interface HostCommandOutput {
  exit_code: number;                  // WIT s32
  stdout: string;
  stderr: string;
  error: string | null;
}

interface ShellExtensionConfig {
  default_timeout_seconds?: number; // 非负 u64；省略为 1800；0 表示禁用
}

interface ShellToolArguments {
  command: string;
  timeout_seconds?: number;         // 非负 u64；省略使用配置默认值；0 表示禁用
}

interface ToolDefinition {
  name: string;
  description: string;
  parameters: JsonValue;             // 必须是顶层 type="object" 的 JSON Schema
}

interface ToolEntry extends ToolDefinition {
  id?: string;                       // 新工具省略；核心分配后必须保留
  owner?: string;                    // 新工具省略；核心管理，扩展不能接管
  enabled?: boolean;                 // 省略时为 true
}

interface ContextView {
  items_count: number;
  recent_messages: Array<{
    role: string;
    content: string;
  }>;
}

interface TurnContext {
  items: Item[];
  view: ContextView;
}

interface AgentDraft {
  system_prompt: string;
  model: ModelDraft;
  tools?: ToolEntry[];               // 省略时为空数组
  context?: TurnContext;             // agent.prepare 中省略；turn.prepare 中存在且只读
}

interface InputPreparePayload {
  text: string;
  delayed: boolean;
}

interface ModelResponsePayload {
  text: string;
  items: Item[];
}

interface ToolCallRequest {
  call_id: string;
  tool_id: string;
  name: string;
  arguments: JsonValue;
}

interface ToolResult {
  success: boolean;
  output: string | InputContent[];
  error?: string | null;
}

interface ToolResultPreparePayload {
  call: ToolCallRequest;
  result: ToolResult;
}

type ContextCommitReason = "input" | "model_response" | "tool_results";

interface ContextCommitPayload {
  reason: ContextCommitReason;
  current: Item[];
  pending: Item[];
  next: Item[];
}

interface TurnCompletePayload {
  iteration: number;
  called_tools: boolean;
  continue_loop: boolean;
}

interface AgentErrorPayload {
  error: string;
}
```

`HostCommandOutput` 不是 JSON Hook payload，而是 `host.execute-command-with-timeout` 返回的 WIT record。上面的 `exit_code` 是伪代码名称；实际生成的语言绑定可能使用 `exitCode`、`exit_code` 等本语言命名方式，对应的 WIT 字段是 `exit-code`。`timeout-ms = 0` 表示不启用超时。`ExtensionConfigItem` 和 `ExtensionsConfig` 描述 TOML 配置反序列化后的逻辑结构，其中只有单个条目的 `config` 会被序列化成 JSON 传给该扩展。

### 4.6 Open Responses 请求结构

`model.request.prepare` 的 payload 是完整的 `CreateResponseBody`。核心初始只设置 `model`、`input`、`instructions`、函数工具、`tool_choice`、`temperature`、`max_output_tokens` 和 `stream=false`，但扩展可以操作以下所有受支持字段：

```ts
type IncludeOption =
  | "reasoning.encrypted_content"
  | "message.output_text.logprobs";

type ToolChoice = "none" | "auto" | "required";
type Truncation = "auto" | "disabled";
type ServiceTier = "auto" | "default" | "flex" | "priority";
type Verbosity = "low" | "medium" | "high";
type ReasoningEffort = "none" | "low" | "medium" | "high" | "xhigh";
type ReasoningSummary = "concise" | "detailed" | "auto";

interface FunctionTool {
  type: "function";
  name: string;
  description?: string;
  parameters?: JsonValue;
  strict?: boolean;
}

interface McpTool {
  type: "mcp";
  server_label: string;
  server_url: string;
  allowed_tools?: string[];
}

interface ExtensionRequestTool {
  type: string;                       // 除 function/mcp 外的类型
  [key: string]: JsonValue;
}

type RequestTool = FunctionTool | McpTool | ExtensionRequestTool;

type ToolChoiceParam =
  | ToolChoice
  | { type: string; name: string }
  | {
      type: string;
      tools: Array<{ type: string; name: string }>;
      mode?: ToolChoice;
    };

type TextFormat =
  | { type: "text" }
  | { type: "json_object" }
  | {
      type: "json_schema";
      name: string;
      description?: string;
      schema?: JsonValue;
      strict?: boolean;
    };

interface TextParam {
  format?: TextFormat;
  verbosity?: Verbosity;              // 反序列化默认 medium
}

interface ReasoningConfig {
  effort?: ReasoningEffort;
  summary?: ReasoningSummary;
}

interface CreateResponseBody {
  model?: string;
  input?: string | Item[];
  previous_response_id?: string;
  include?: IncludeOption[];
  tools?: RequestTool[];
  tool_choice?: ToolChoiceParam;
  metadata?: { [key: string]: string };
  text?: TextParam;
  temperature?: number;
  top_p?: number;
  presence_penalty?: number;
  frequency_penalty?: number;
  parallel_tool_calls?: boolean;
  stream?: boolean;
  stream_options?: { include_obfuscation?: boolean | null };
  background?: boolean;
  max_output_tokens?: number;
  max_tool_calls?: number;
  reasoning?: ReasoningConfig;
  safety_identifier?: string;
  prompt_cache_key?: string;
  truncation?: Truncation;             // 省略反序列化为 auto；核心序列化时存在
  instructions?: string;
  store?: boolean;
  service_tier?: ServiceTier;          // 省略反序列化为 auto；核心序列化时存在
  top_logprobs?: number;
}
```

返回的请求必须能反序列化为上述结构。核心额外要求 `model` 存在且非空、`temperature` 合法、`max_output_tokens` 为正数。Agent 使用同步非流式模型 I/O，因此会拒绝 `stream=true` 和 `background=true`。

### 4.7 Context Item 完整结构

`AgentDraft.context.items`、`model.response.prepare.items` 和 `context.commit` 都使用以下 `Item` 联合类型。所有 Item 都通过字符串 `type` 区分：

```ts
type ItemStatus = "in_progress" | "completed" | "incomplete";
type MessageRole = "user" | "assistant" | "system" | "developer";
type MessagePhase = "commentary" | "final_answer";
type ImageDetail = "low" | "high" | "auto";

interface UrlCitation {
  type: "url_citation";
  url: string;
  title: string;
  start_index: number;
  end_index: number;
}

interface TopLogProb {
  token: string;
  logprob: number;
  bytes: number[];
}

interface LogProb extends TopLogProb {
  top_logprobs: TopLogProb[];
}

type InputContent =
  | { type: "input_text"; text: string }
  | { type: "input_image"; image_url?: string; detail?: ImageDetail }
  | { type: "input_file"; filename?: string; file_data?: string; file_url?: string }
  | { type: "input_video"; video_url: string };

type MessageContent =
  | InputContent
  | {
      type: "output_text";
      text: string;
      annotations?: UrlCitation[];
      logprobs?: LogProb[];
    }
  | { type: "refusal"; refusal: string }
  | { type: "text"; text: string }
  | { type: "summary_text"; text: string }
  | { type: "reasoning_text"; text: string };

interface MessageItem {
  type: "message";
  id?: string;
  status?: ItemStatus;
  role: MessageRole;
  content: MessageContent[] | string;  // 序列化输出始终为数组
  phase?: MessagePhase;
}

interface FunctionCallItem {
  type: "function_call";
  id?: string;
  call_id: string;
  name: string;
  arguments: string;                   // JSON 编码后的字符串，不是 JSON 对象
  status?: ItemStatus;
}

interface FunctionCallOutputItem {
  type: "function_call_output";
  id?: string;
  call_id: string;
  output: string | InputContent[];
  status?: ItemStatus;
}

interface ReasoningItem {
  type: "reasoning";
  id?: string;
  status?: ItemStatus;
  content?: MessageContent[];
  summary: MessageContent[];
  encrypted_content?: string;
}

interface CompactionItem {
  type: "compaction";
  id?: string;
  encrypted_content: string;
  created_by?: string;
}

interface ItemReference {
  type: "item_reference";
  id: string;
}

interface ExtensionItem {
  type: string;                         // 除上述内置类型外的任意类型
  id?: string;
  status?: string;
  [key: string]: JsonValue;
}

type Item =
  | MessageItem
  | FunctionCallItem
  | FunctionCallOutputItem
  | ReasoningItem
  | CompactionItem
  | ItemReference
  | ExtensionItem;
```

Message 内容还受 role 约束：`user` 只允许四种 `input_*`；`system` 和 `developer` 只允许 `input_text`；`assistant` 只允许 `output_text` 或 `refusal`。`FunctionCallOutputItem.output` 为数组时只允许四种 `input_*`。`context.commit.next` 不允许 `role="system"` 的 Message；System Prompt 必须通过 AgentDraft 或请求 `instructions` 修改。

### 4.8 Observer 事件结构

```ts
interface ModelResponseObservePayload {
  id: string;
  status: "queued" | "in_progress" | "completed" | "failed" | "incomplete" | string;
  output: Item[];
  error: { code?: string; message: string; param?: string; type?: string } | null;
  incomplete_details?: { reason: string };
  usage?: JsonValue;
  [key: string]: JsonValue;
}

type AgentShutdownPayload = Record<string, never>; // JSON {}
```

Hook 与 payload 类型的完整映射：

```ts
interface HookPayloadMap {
  "agent.prepare": AgentDraft;
  "input.prepare": InputPreparePayload;
  "turn.prepare": AgentDraft;
  "model.request.prepare": CreateResponseBody;
  "model.response.observe": ModelResponseObservePayload;
  "model.response.prepare": ModelResponsePayload;
  "tool.call.prepare": ToolCallRequest;
  "tools.call": ToolCallRequest;                 // Action 返回 ToolResult
  "tool.result.prepare": ToolResultPreparePayload;
  "context.commit": ContextCommitPayload;
  "turn.complete": TurnCompletePayload;
  "agent.error": AgentErrorPayload;
  "agent.shutdown": AgentShutdownPayload;
}
```

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
  "tools": []
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
  -> 非流式模型 I/O
  -> model.response.observe（完整响应资源）
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
取消     -> 丢弃当前模型 / Hook / 工具 Future
关闭     -> agent.shutdown -> lifecycle.shutdown
```

宿主持有一个共享取消令牌。CLI 的 `Ctrl+C` 与 `AgentSender::cancel()` 都会取消
正在运行的 Agent Future；取消属于正常结束，不触发 `agent.error`。宿主随后调用
`agent.shutdown` 和生命周期 `shutdown`。被取消的 `invoke` Future 可能在任意 await
点被丢弃，因此扩展不能假定单次调用内的清理代码一定执行完毕；持久资源清理应放在
生命周期 `shutdown` 中。宿主命令会在被取消的 Future 丢弃时终止对应子进程。

`BaseAgentState` 不会被每轮动态变更永久污染。若要永久添加工具或修改默认 Prompt，在 `agent.prepare` 修改；若要按上下文动态变化，在 `turn.prepare` 修改。

## 7. Hook 点位与载荷

下表中的“返回”指 `HookResult.payload`。`unchanged` 不需要携带 payload。

| Hook | 类型 | 时机 | 输入/返回 payload | skip / stop |
| --- | --- | --- | --- | --- |
| `agent.prepare` | Transform | 初始化扩展后 | `AgentDraft -> AgentDraft` | 两者都会使 Agent 初始化失败 |
| `input.prepare` | Transform | 用户输入提交前 | `{text:string, delayed:bool}` | skip 丢弃输入；stop 停止运行 |
| `turn.prepare` | Transform | 每轮构造请求前 | `AgentDraft -> AgentDraft`，含只读 context | skip 跳过本轮；stop 结束 loop |
| `model.request.prepare` | Transform | 调用模型前 | Open Responses `CreateResponseBody` JSON | skip 跳过本轮；stop 结束 loop |
| `model.response.observe` | Observer | 响应解析后、状态处理前 | 完整的 Open Responses `ResponseResource` JSON | 不适用 |
| `model.response.prepare` | Transform | 得到可用响应后、提交前 | `{text:string, items:Item[]}` | skip 丢弃响应并进入下一轮；stop 结束 loop |
| `tool.call.prepare` | Transform | Action 前 | `ToolCallRequest -> ToolCallRequest` | skip 生成“已跳过”工具输出；stop 结束 loop |
| `tools.call` | Action | 实际执行工具 | `ToolCallRequest -> ToolResult` | Action 不允许 skip/stop |
| `tool.result.prepare` | Transform | 工具结果提交前 | `{call:ToolCallRequest,result:ToolResult}` | 当前 skip/stop 均保留进入 Hook 前的结果 |
| `context.commit` | Transform | 任意上下文写入前 | `ContextCommitDraft -> ContextCommitDraft` | 不提交；调用方停止或跳过对应阶段 |
| `turn.complete` | Transform | 一轮结束时 | `{iteration,called_tools,continue_loop}` | skip 保持原决定；stop 结束 loop |
| `agent.error` | Observer | `run()` 返回错误时 | `{error:string}` | 不适用；Observer 错误在此处不会覆盖原错误 |
| `agent.shutdown` | Observer | 生命周期 shutdown 前 | `{}` | 不适用；即使 Observer 报错仍会调用 lifecycle shutdown |

### 7.1 model.response.observe

HTTP 请求和 JSON 解码成功后，此 Observer 触发一次。payload 是完整的 Open Responses `ResponseResource`，包含 `status`、`output`、`error`、`incomplete_details` 和 `usage`。它在核心拒绝失败响应之前触发，因此遥测扩展可以检查模型级失败。传输、HTTP 和 JSON 解码失败没有响应资源，改由 `agent.error` 报告。

### 7.2 ToolCallRequest / ToolResult

```json
{
  "call_id": "call_123",
  "tool_id": "tool-1",
  "name": "shell",
  "arguments": {"command":"pwd","timeout_seconds":30}
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

或者返回结构化多模态内容（如图片）：

```json
{
  "action": "continue",
  "payload": {
    "success": true,
    "output": [
      { "type": "input_text", "text": "Image Metadata:\n..." },
      { "type": "input_image", "image_url": "data:image/png;base64,..." }
    ],
    "error": null
  }
}
```

- `output`：工具执行结果内容，既支持直接返回纯文本字符串 `string`，也支持返回多模态结构化输入内容数组 `InputContent[]`（如 `input_text`、`input_image`、`input_file`、`input_video`）。核心在生成 `function_call_output` 时会直接转换为 `FunctionOutput`（`Text` 或 `Content`）提交给模型上下文，并在事件日志中自动提取摘要文本展示。
- `error`（可选）：失败原因。

工具失败也应返回正常的 HookResult，并令 `success = false`；只有扩展协议本身失败时才返回 WIT `err(string)`。

仓库内置的 Shell 示例中，`timeout_seconds` 是可选参数。扩展依次取单次参数、
初始化配置中的 `default_timeout_seconds`、最后取 1800 秒；值为 `0` 表示禁用超时。
工具 JSON Schema 中显示的 `default` 只用于提示，实际执行时扩展仍会重复完成兜底。
正数会转换为毫秒并传给 `host.execute-command-with-timeout`。命令超时会返回正常的
失败 `ToolResult`；即使禁用了命令超时，`Ctrl+C` 全局取消仍然有效。

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

Rust 是当前仓库完整验证的开发路径。最简单的方式是复制 [`extensions/shell`](extensions/shell)、[`extensions/file_editor`](extensions/file_editor) 或 [`extensions/image_viewer`](extensions/image_viewer)。

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

使用 Rust 的 WASIp2 target 直接构建 Component：

```sh
cargo build --target wasm32-wasip2 --release
```

生成的 `target/wasm32-wasip2/release/example.wasm` 已经是 Component。仓库脚本 [`scripts/build-extensions.sh`](scripts/build-extensions.sh) 使用这条路径构建两个示例扩展。

仓库提供的 [`scripts/install-extensions.sh`](scripts/install-extensions.sh) 会先执行上述
构建，再原子安装或更新选中的 `.wasm` 文件。如果 `config.toml` 缺少选中扩展的条目，脚本会通过同目录临时文件原子追加，不修改已存在的扩展条目。

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

ragent 会链接项目的 `ragent:extension/host` 接口和 `wasmtime-wasi` 提供的 WASI 0.2 接口。交付前应用下文的 `wasm-tools component wit` 检查最终 imports；不在这两类集合内的 import（包括 `wasi:http`）仍会在加载时失败。

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

ragent 提供 `wasmtime-wasi` 中的核心 WASI 0.2 接口，但不提供 `wasi:http`。ComponentizeJS 应将产物 imports 限制为项目 world 和宿主已支持的 WASI 0.2 接口；除非宿主新增对应支持，应关闭 `http` 和 `fetch-event`。

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

如果生成的 Component 依赖宿主支持集合之外的 WASI imports，应使用 `--stub-wasi`；加载前始终检查最终 imports。Python 会把解释器和应用一起打进 Component，因此产物通常明显大于 Rust 扩展，加载时间也更长。

## 12. 检查 Component

无论使用哪种语言，最终文件必须是 Component，不是普通 core Wasm：

```sh
wasm-tools validate extension.wasm --features component-model
wasm-tools component wit extension.wasm
```

提取出的 world 必须兼容 `ragent:extension/plugin@1.0.0`。宿主会链接 `ragent:extension/host` 和 `wasmtime-wasi` 提供的 WASI 0.2 接口；任何其他 import（包括 `wasi:http`）目前都无法实例化。

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
name = "shell"
path = "extensions/shell.wasm"
enabled = true

[extensions.config]
default_timeout_seconds = 1800

[[extensions]]
name = "file_editor"
path = "extensions/file_editor.wasm"
enabled = true

[[extensions]]
name = "image_viewer"
path = "extensions/image_viewer.wasm"
enabled = true
```

Shell 扩展接受 `default_timeout_seconds`：省略时为 1800 秒（30 分钟），设为
`0` 时禁用命令超时。工具调用可用 `timeout_seconds` 覆盖配置值。

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
- 响应状态、用量统计和模型错误遥测使用 `model.response.observe`；只有需要修改响应内容时才使用 `model.response.prepare`。
- 工具扩展必须测试成功、参数错误、宿主错误、非零退出码四类结果。
- 不要依赖另一个扩展生成的临时工具 ID；ID 只保证当前 Agent 进程和当前草稿链路内稳定。
- 若 Component 能通过 `wasm-tools validate` 但无法加载，首先检查它是否含有宿主未链接的额外 imports。
