**English** | [简体中文](EXTENSIONS.zh-CN.md)

# ragent extension development and Hook protocol

This document describes the WASM Component extension protocol implemented by the current code. See [`wit/ragent-extension.wit`](wit/ragent-extension.wit) for the ABI, [`src/wasm/types.rs`](src/wasm/types.rs) for the Rust data structures, and the complete examples in [`extensions/shell`](extensions/shell), [`extensions/file_editor`](extensions/file_editor), and [`extensions/image_viewer`](extensions/image_viewer).

## 1. Design goals

The Agent core only owns model I/O, context commits, and loop control. Extensions can modify:

- the System Prompt;
- model name, temperature, and maximum output tokens;
- tool definitions, tool arguments, and tool results;
- user input, model requests, and model responses;
- pending context commits;
- the decision to continue the loop.

WASM is a cross-language extension format here, not a security sandbox. Host capabilities are determined by WIT imports. The project host interface can execute commands through `sh -c`, and the runtime links supported WASI 0.2 interfaces. Do not load untrusted extensions.

## 2. Component ABI

Every extension must implement the `plugin` world:

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

Lifecycle methods:

| Method | Calls | Purpose |
| --- | ---: | --- |
| `metadata` | Once while loading | Returns extension identity and Hook subscriptions as JSON |
| `initialize` | Once during Agent initialization | Receives the matching `[extensions.config]` value as JSON |
| `invoke` | Once per Hook invocation | Receives HookRequest JSON and returns HookResult JSON or an error string |
| `shutdown` | Once while shutting down | Releases extension state, including after graceful cancellation |

One JSON-based `invoke` method keeps the Component ABI stable when new Hooks are added. The current JSON protocol is `protocol_version = 1`.

## 3. Metadata

`metadata()` returns:

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

Rules:

- `id` is the real extension identity. It must be non-empty and unique within one load.
- `version` is the extension version, not the Hook protocol version.
- `hook` uses one of the Hook names documented below.
- `kind` is `transform`, `action`, or `observer`.
- Lower `priority` values run first. Configuration order breaks ties.
- `failure` is `abort` or `ignore`.
- One extension cannot declare the same Hook and kind more than once.

The configuration entry's `name` is a loader-facing label. Identity, ownership, and duplicate checks use metadata `id`.

## 4. Common request and result envelopes

### 4.1 HookRequest

`invoke()` receives a JSON string with this shape:

```json
{
  "hook": "turn.prepare",
  "protocol_version": 1,
  "invocation_id": 42,
  "iteration": 2,
  "payload": {}
}
```

- `invocation_id` increases monotonically within the process and can correlate logs.
- `iteration` is present for turn-scoped Hooks. Initialization, input, and shutdown Hooks may omit it.
- `payload` is defined by the Hook point.

Reject unsupported `protocol_version` values instead of guessing their structure.

### 4.2 HookResult

Transform and Action return one envelope:

```json
{
  "action": "continue",
  "payload": {},
  "reason": null
}
```

| action | Transform | Action |
| --- | --- | --- |
| `continue` | Replace the current draft with `payload` and continue | Return the Action result; `payload` is required |
| `unchanged` | Keep the input and continue | Not allowed |
| `reject` | Reject with `reason` and terminate | Reject with `reason` and terminate |
| `skip` | Skip the stage; exact behavior depends on the Hook | Not allowed |
| `stop` | Stop the surrounding flow; exact behavior depends on the Hook | Not allowed |

Observer results are ignored, but the successful branch of `invoke()` must still return a valid JSON string, such as `{"action":"unchanged"}`.

### 4.3 Dispatch kinds

#### Transform

Transforms run serially by priority. Each extension receives the previous extension's already validated result. A Transform can contribute to an initially empty list, so there is no separate Provider kind.

Every result is validated immediately:

- `failure = "abort"`: fail with the extension ID and Hook name.
- `failure = "ignore"`: discard this extension's result, roll back to its input, and continue.

`reject` covers gating, so there is no separate Gate kind.

#### Action

Actions perform side effects. The core routes an Action to exactly one immutable owner; Actions are not broadcast. The current Action Hook is `tools.call`.

#### Observer

Observers are broadcast concurrently. Their returned values never change the flow. Invocation errors propagate with `abort` and are discarded with `ignore`.

### 4.4 JSON type notation

The following sections use TypeScript syntax to describe wire JSON. Extensions do not need to be written in TypeScript:

- `field?: T` means that the field may be omitted.
- `T | null` means that an explicitly present field may contain JSON `null`.
- `JsonValue` means any valid JSON value.
- Integers must fit the range stated by their target field. JavaScript implementations should treat `invocation_id` as a safe integer.
- Fields not marked optional are required. Do not assume that unknown fields are preserved unless the type explicitly contains `[key: string]`.

```ts
type JsonPrimitive = null | boolean | number | string;
type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };

type HookKind = "transform" | "action" | "observer";
type HookFailurePolicy = "abort" | "ignore";
type HookAction = "continue" | "unchanged" | "reject" | "skip" | "stop";

interface HookSubscription {
  hook: string;
  kind: HookKind;
  priority?: number;             // defaults to 0; signed 32-bit integer
  failure?: HookFailurePolicy;   // defaults to "abort"
}

interface ExtensionMetadata {
  id: string;
  version: string;
  subscriptions?: HookSubscription[]; // defaults to []
}

interface ExtensionConfigItem {
  name: string;
  path: string;
  enabled?: boolean;             // defaults to true
  config?: JsonValue;            // defaults to null; passed to initialize
}

interface ExtensionsConfig {
  extensions?: ExtensionConfigItem[]; // defaults to []
}

interface HookRequest<P = JsonValue> {
  hook: string;
  protocol_version: 1;
  invocation_id: number;         // host u64; correlation identifier
  iteration?: number;            // present only for turn-scoped Hooks
  payload: P;
}

interface HookResult<P = JsonValue> {
  action: HookAction;
  payload?: P;                   // required for continue
  reason?: string;               // recommended for reject
}
```

`metadata()` returns an `ExtensionMetadata` JSON string. `initialize()` receives a JSON string containing any `JsonValue`. `invoke()` receives a `HookRequest` string and returns a `HookResult` string in the WIT `ok` branch.

### 4.5 ragent core data structures

```ts
interface ModelDraft {
  name: string;
  temperature?: number | null;       // present in core output; null or 0..=2
  max_output_tokens?: number | null; // present in core output; null or positive i32
}

interface HostCommandOutput {
  exit_code: number;                  // WIT s32
  stdout: string;
  stderr: string;
  error: string | null;
}

interface ShellExtensionConfig {
  default_timeout_seconds?: number; // non-negative u64; omitted = 1800; 0 disables
}

interface ShellToolArguments {
  command: string;
  timeout_seconds?: number;         // non-negative u64; omitted = configured default; 0 disables
}

interface ToolDefinition {
  name: string;
  description: string;
  parameters: JsonValue;             // JSON Schema with top-level type="object"
}

interface ToolEntry extends ToolDefinition {
  id?: string;                       // omit for new tools; preserve after core assignment
  owner?: string;                    // omit for new tools; managed by the core
  enabled?: boolean;                 // defaults to true
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
  tools?: ToolEntry[];               // defaults to []
  context?: TurnContext;             // omitted in agent.prepare; present and read-only in turn.prepare
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

`HostCommandOutput` is not a JSON Hook payload; it is the WIT record returned by `host.execute-command-with-timeout`. `exit_code` above is pseudocode. Generated bindings may call it `exitCode`, `exit_code`, or another language-native spelling; the WIT field is `exit-code`. `timeout-ms = 0` means no timeout. `ExtensionConfigItem` and `ExtensionsConfig` describe the logical structure deserialized from TOML. Only an individual entry's `config` value is serialized to JSON and passed to that extension.

### 4.6 Open Responses request structures

The `model.request.prepare` payload is a complete `CreateResponseBody`. The core initially sets only `model`, `input`, `instructions`, function tools, `tool_choice`, `temperature`, `max_output_tokens`, and `stream=false`, but an extension may operate on every supported field below:

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
  type: string;                       // any type other than function/mcp
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
  verbosity?: Verbosity;              // deserialization default: medium
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
  truncation?: Truncation;             // omitted input defaults to auto; present in core output
  instructions?: string;
  store?: boolean;
  service_tier?: ServiceTier;          // omitted input defaults to auto; present in core output
  top_logprobs?: number;
}
```

The result must deserialize to this structure. The core additionally requires a present, non-empty `model`, valid `temperature`, and positive `max_output_tokens`. The Agent performs synchronous, non-streaming model I/O, so `stream=true` and `background=true` are rejected.

### 4.7 Complete Context Item structure

`AgentDraft.context.items`, `model.response.prepare.items`, and `context.commit` use this `Item` union. Every Item is discriminated by the string field `type`:

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
  content: MessageContent[] | string;  // serialized output is always an array
  phase?: MessagePhase;
}

interface FunctionCallItem {
  type: "function_call";
  id?: string;
  call_id: string;
  name: string;
  arguments: string;                   // JSON-encoded string, not a JSON object
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
  type: string;                         // any type other than the built-in variants above
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

Message content is also role-constrained: `user` permits the four `input_*` variants; `system` and `developer` permit only `input_text`; `assistant` permits only `output_text` or `refusal`. When `FunctionCallOutputItem.output` is an array, it may contain only the four `input_*` variants. A `context.commit.next` value cannot contain a Message with `role="system"`; modify the System Prompt through AgentDraft or request `instructions`.

### 4.8 Observer event structures

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

Complete Hook-to-payload mapping:

```ts
interface HookPayloadMap {
  "agent.prepare": AgentDraft;
  "input.prepare": InputPreparePayload;
  "turn.prepare": AgentDraft;
  "model.request.prepare": CreateResponseBody;
  "model.response.observe": ModelResponseObservePayload;
  "model.response.prepare": ModelResponsePayload;
  "tool.call.prepare": ToolCallRequest;
  "tools.call": ToolCallRequest;                 // Action result: ToolResult
  "tool.result.prepare": ToolResultPreparePayload;
  "context.commit": ContextCommitPayload;
  "turn.complete": TurnCompletePayload;
  "agent.error": AgentErrorPayload;
  "agent.shutdown": AgentShutdownPayload;
}
```

## 5. AgentDraft and tool ownership

`agent.prepare` and `turn.prepare` share one AgentDraft shape:

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

In `turn.prepare`, `context` is read-only:

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

An extension can use complete `items` or the compact `view` to change tools, the Prompt, or model parameters dynamically. The core restores `context` to its input value after each Transform. Use `context.commit` when modifying context.

### 5.1 Adding a tool

A new tool must omit `id` and `owner`:

```json
{
  "enabled": true,
  "name": "weather",
  "description": "Look up the weather",
  "parameters": {
    "type": "object",
    "properties": {
      "city": {"type": "string"}
    },
    "required": ["city"]
  }
}
```

The core supplies both fields before the next extension runs:

```json
{
  "id": "tool-1",
  "owner": "weather-extension",
  "enabled": true,
  "name": "weather",
  "description": "Look up the weather",
  "parameters": {"type": "object"}
}
```

Ownership rules:

- An extension adding a tool must also subscribe to `tools.call` as an Action.
- A new tool cannot supply an ID. Unknown supplied IDs are rejected.
- An existing tool keeps its ID. The core restores its Owner, preventing takeover.
- Tools may be modified, disabled, deleted, or reordered.
- All names and IDs must be unique, including disabled tools.
- The top-level `parameters.type` must currently be `object`.

### 5.2 Model validation

- `system_prompt` must not be empty.
- The model name must not be empty.
- `temperature` is null or within `0..=2`.
- `max_output_tokens` is null or greater than zero.

## 6. Agent flow

```text
read ~/.config/ragent/config.toml
  -> load Components / metadata
  -> validate subscriptions
  -> initialize
  -> agent.prepare
  -> save BaseAgentState

input
  -> input.prepare
  -> context.commit(reason=input)

each turn
  -> clone BaseAgentState + complete current context
  -> turn.prepare
  -> model.request.prepare
  -> non-streaming model I/O
  -> model.response.observe with the complete response resource
  -> model.response.prepare
  -> context.commit(reason=model_response)
  -> when tool calls exist:
       tool.call.prepare
       -> tools.call routed to the tool Owner
       -> tool.result.prepare
       -> context.commit(reason=tool_results)
  -> turn.complete
  -> continue or finish

run error -> agent.error
cancel    -> drop current model / Hook / tool future
shutdown  -> agent.shutdown -> lifecycle.shutdown
```

The host owns a shared cancellation token. CLI `Ctrl+C` and
`AgentSender::cancel()` cancel the active Agent future; cancellation is a normal
completion and does not invoke `agent.error`. The host then invokes
`agent.shutdown` and lifecycle `shutdown`. A canceled `invoke` future may be
dropped at any await point, so extensions must not rely on per-invocation cleanup
running to completion. Put durable cleanup in lifecycle `shutdown`. Host commands
are configured to kill their child process when the canceled future is dropped.

Per-turn changes never mutate `BaseAgentState`. Use `agent.prepare` for persistent default tools or Prompt changes. Use `turn.prepare` for context-dependent changes.

## 7. Hook points and payloads

“Result” below means `HookResult.payload`. `unchanged` does not need a payload.

| Hook | Kind | Timing | Input/result payload | skip / stop |
| --- | --- | --- | --- | --- |
| `agent.prepare` | Transform | After extension initialization | `AgentDraft -> AgentDraft` | Both make Agent initialization fail |
| `input.prepare` | Transform | Before user input is committed | `{text:string, delayed:bool}` | skip drops input; stop stops the run |
| `turn.prepare` | Transform | Before each model request is built | `AgentDraft -> AgentDraft`, with read-only context | skip skips the turn; stop exits the loop |
| `model.request.prepare` | Transform | Immediately before model I/O | Open Responses `CreateResponseBody` JSON | skip skips the turn; stop exits the loop |
| `model.response.observe` | Observer | After response parsing, before status handling | Complete Open Responses `ResponseResource` JSON | Not applicable |
| `model.response.prepare` | Transform | After a usable response, before commit | `{text:string, items:Item[]}` | skip discards and retries next turn; stop exits |
| `tool.call.prepare` | Transform | Before the Action | `ToolCallRequest -> ToolCallRequest` | skip emits a skipped output; stop exits |
| `tools.call` | Action | Actual tool execution | `ToolCallRequest -> ToolResult` | Actions reject skip/stop |
| `tool.result.prepare` | Transform | Before committing tool output | `{call:ToolCallRequest,result:ToolResult}` | Current skip/stop retain the pre-Hook result |
| `context.commit` | Transform | Before any context write | `ContextCommitDraft -> ContextCommitDraft` | Does not commit; caller stops or skips its stage |
| `turn.complete` | Transform | At the end of a turn | `{iteration,called_tools,continue_loop}` | skip retains the original decision; stop exits |
| `agent.error` | Observer | When `run()` returns an error | `{error:string}` | Observer errors do not replace the original error |
| `agent.shutdown` | Observer | Before lifecycle shutdown | `{}` | Lifecycle shutdown still runs if the Observer fails |

### 7.1 model.response.observe

This Observer runs once after a successful HTTP request and JSON decode. Its payload is the complete Open Responses `ResponseResource`, including `status`, `output`, `error`, `incomplete_details`, and `usage`. It runs before the core rejects a failed response, so telemetry extensions can inspect model-level failures. Transport, HTTP, and JSON decoding failures have no response resource and are reported through `agent.error` instead.

### 7.2 ToolCallRequest and ToolResult

```json
{
  "call_id": "call_123",
  "tool_id": "tool-1",
  "name": "shell",
  "arguments": {"command":"pwd","timeout_seconds":30}
}
```

`tool.call.prepare` may change `arguments`. It cannot change `call_id` or `tool_id`. The core restores `name` from the ToolEntry before routing.

The Action returns:

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

Or with structured multimodal content (e.g. image viewing):

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

- `output`: The execution output of the tool. It supports either a plain text `string` or an allowed multimodal input content array `InputContent[]` (such as `input_text`, `input_image`, `input_file`, `input_video`). The core automatically maps it to `FunctionOutput` (`Text` or `Content`) in `function_call_output` for model context, while formatting text for console event logs.
- `error` (optional): Error description if tool execution failed.

A normal tool failure is still a successful WIT invocation with `success = false`. Return WIT `err(string)` only when the extension protocol itself fails.

For the bundled Shell extension, `timeout_seconds` is optional. The extension
resolves the per-call value first, then `default_timeout_seconds` from its
initialization config, then 1,800 seconds. A value of `0` disables the timeout.
The displayed JSON Schema `default` is informational; the extension repeats this
fallback at execution time. Positive values are converted to milliseconds and
passed to `host.execute-command-with-timeout`. Timeout returns a normal failed
`ToolResult`, while `Ctrl+C` cancellation remains effective even when timeout is
disabled.

### 7.3 context.commit

```json
{
  "reason": "model_response",
  "current": [],
  "pending": [],
  "next": []
}
```

- `reason` is `input`, `model_response`, or `tool_results`.
- `current` is context before the commit.
- `pending` contains Items about to be appended.
- `next` defaults to `current + pending`; modify and return `next`.

The core deserializes `next` and checks that System Messages stay outside Items, Function Call IDs are unique, and Function Call Outputs reference an earlier Call ID. The System Prompt is submitted separately through model request `instructions`.

## 8. Developing in Rust

Rust is the fully verified path in this repository. The easiest starting point is [`extensions/shell`](extensions/shell), [`extensions/file_editor`](extensions/file_editor), or [`extensions/image_viewer`](extensions/image_viewer).

`Cargo.toml`:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
wit-bindgen = "0.46"
```

Minimal entry point:

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

Build the Component directly with Rust's WASIp2 target:

```sh
cargo build --target wasm32-wasip2 --release
```

The resulting `target/wasm32-wasip2/release/example.wasm` is already a Component. [`scripts/build-extensions.sh`](scripts/build-extensions.sh) uses this path for both bundled extensions.

For the bundled extensions, [`scripts/install-extensions.sh`](scripts/install-extensions.sh)
runs that build and atomically installs or updates the selected `.wasm` files.
It also atomically appends any missing selected-extension entries to `config.toml`
without changing entries that already exist.

## 9. Developing in Go

Use Bytecode Alliance [`componentize-go`](https://github.com/bytecodealliance/componentize-go) or `wit-bindgen go`. These tools and generated APIs are evolving, so check the locally installed version's `--help` and follow its generated signatures.

Basic setup:

```sh
go mod init example.com/ragent-extension
go install github.com/bytecodealliance/componentize-go@latest
componentize-go --help
```

Copy or link this project's `wit/` into the Go module, select world `plugin`, and generate bindings. Implement `Metadata`, `Initialize`, `Invoke`, and `Shutdown` in the generated lifecycle export package. Hook business data remains JSON.

The lower-level build path is:

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

ragent links the project `ragent:extension/host` interface and the WASI 0.2 interfaces provided by `wasmtime-wasi`. Inspect the final imports with `wasm-tools component wit`; imports outside those sets, including `wasi:http`, still fail during loading.

## 10. Developing in AssemblyScript

AssemblyScript produces core Wasm, but there is currently no first-class WIT Component binding generator directly usable by this project. A `.wasm` produced by `asc` is not a Component and cannot be configured directly.

An experimental path requires you to:

1. Implement lifecycle exports and the host import in AssemblyScript.
2. Implement WIT Canonical ABI lowering/lifting for strings, records, and results.
3. Embed the `plugin` world type into core Wasm.
4. Wrap it with `wasm-tools component new`.

```sh
npx asc assembly/index.ts -o core.wasm
wasm-tools component embed -w plugin /path/to/ragent/wit \
  core.wasm -o core-with-wit.wasm
wasm-tools component new core-with-wit.wasm -o extension.wasm
```

These commands do not generate the Canonical ABI. Without step 2, conversion or instantiation fails.

The practical TypeScript alternative is to compile application logic to ESM JavaScript and use Bytecode Alliance [`ComponentizeJS`](https://github.com/bytecodealliance/ComponentizeJS). This is not an AssemblyScript binary pipeline, but it generates a Component directly from TypeScript/JavaScript logic.

ragent provides the core WASI 0.2 interfaces from `wasmtime-wasi`, but not `wasi:http`. Configure ComponentizeJS so that the generated imports are limited to the project world and supported WASI 0.2 interfaces; disable `http` and `fetch-event` unless the host gains those interfaces.

## 11. Developing in Python

Use Bytecode Alliance [`componentize-py`](https://github.com/bytecodealliance/componentize-py), which requires Python 3.10 or newer:

```sh
python3 -m venv .venv
. .venv/bin/activate
pip install componentize-py
componentize-py -d /path/to/ragent/wit -w plugin \
  bindings ragent_guest
```

Implement the four methods using the generated lifecycle base class:

- `metadata()` returns a JSON string.
- `initialize(config)` parses initialization JSON.
- `invoke(request)` parses HookRequest and returns a HookResult JSON string.
- `shutdown()` releases state.

Build:

```sh
componentize-py -d /path/to/ragent/wit -w plugin \
  componentize --stub-wasi app -o extension.wasm
```

Use `--stub-wasi` when the generated Component requires interfaces outside the host's supported WASI 0.2 set. Always inspect the final imports before loading. Python packages the interpreter with the application, so its Component is normally much larger and slower to load than a Rust extension.

## 12. Inspecting a Component

The final file must be a Component, not a core Wasm module:

```sh
wasm-tools validate extension.wasm --features component-model
wasm-tools component wit extension.wasm
```

The extracted world must be compatible with `ragent:extension/plugin@1.0.0`. The host links `ragent:extension/host` plus the WASI 0.2 interfaces supplied by `wasmtime-wasi`; any other import, including `wasi:http`, cannot currently be instantiated.

Test loading with a temporary configuration:

```toml
[[extensions]]
name = "example"
path = "/absolute/path/to/extension.wasm"
enabled = true
```

## 13. Installing and loading extensions

Default layout:

```text
~/.config/ragent/
├── config.toml
└── extensions/
    ├── shell.wasm
    └── example.wasm
```

`config.toml`:

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

The Shell extension accepts `default_timeout_seconds`. Omit it for 1,800 seconds
(30 minutes), or set it to `0` to disable command timeout. Tool calls can override
the configured value with `timeout_seconds`.

Loading rules:

- The default directory is `~/.config/ragent/`.
- With `XDG_CONFIG_HOME`, it is `$XDG_CONFIG_HOME/ragent/`.
- Relative Component paths are resolved from the configuration directory.
- When configuration is absent, the Agent creates the directory but enables no extension.
- Entries with `enabled = false` are not loaded.
- Extensions load and initialize in configuration order.
- A missing enabled file, invalid metadata, or initialization failure aborts Agent initialization.
- Configuration changes require an Agent restart; hot reload is not implemented.

Multiple extensions:

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

When multiple extensions subscribe to a Transform, execution is ordered first by `priority`, then by configuration order.

## 14. Debugging guidance

- Start with one metadata subscription and add Hooks after loading succeeds.
- Use `failure = "abort"` while developing. Consider `ignore` later for optional enhancements.
- Log `invocation_id`, `hook`, and `iteration`, but never API keys or sensitive context.
- Keep Transforms close to pure functions. Put side effects in Actions or Observers.
- Use `model.response.observe` for response status, usage accounting, and model-error telemetry; use `model.response.prepare` only when the response content must be transformed.
- Test successful execution, invalid arguments, host errors, and non-zero exit status for every tool.
- Do not persist another extension's temporary tool ID. It is stable only within the current Agent process and draft chain.
- If `wasm-tools validate` succeeds but loading fails, inspect unsupported extra imports first.
