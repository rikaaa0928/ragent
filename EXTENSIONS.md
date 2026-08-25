**English** | [简体中文](EXTENSIONS.zh-CN.md)

# ragent extension development and Hook protocol

This document describes the WASM Component extension protocol implemented by the current code. See [`wit/ragent-extension.wit`](wit/ragent-extension.wit) for the ABI, [`src/wasm/types.rs`](src/wasm/types.rs) for the Rust data structures, and [`extensions/shell`](extensions/shell) for a complete working extension.

## 1. Design goals

The Agent core only owns model I/O, context commits, and loop control. Extensions can modify:

- the System Prompt;
- model name, temperature, and maximum output tokens;
- tool definitions, tool arguments, and tool results;
- user input, model requests, and model responses;
- pending context commits;
- the decision to continue the loop.

WASM is a cross-language extension format here, not a security sandbox. Host capabilities are determined by WIT imports. The current host provides only `host.execute-command`, which executes commands directly on the host through `sh -c`. Do not load an untrusted Shell extension.

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

Lifecycle methods:

| Method | Calls | Purpose |
| --- | ---: | --- |
| `metadata` | Once while loading | Returns extension identity and Hook subscriptions as JSON |
| `initialize` | Once during Agent initialization | Receives the matching `[extensions.config]` value as JSON |
| `invoke` | Once per Hook invocation | Receives HookRequest JSON and returns HookResult JSON or an error string |
| `shutdown` | Once while shutting down | Releases extension state |

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
  "tools": [],
  "context": null
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
  -> streaming model I/O
       -> model.stream.observe for every normalized event
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
shutdown  -> agent.shutdown -> lifecycle.shutdown
```

Per-turn changes never mutate `BaseAgentState`. Use `agent.prepare` for persistent default tools or Prompt changes. Use `turn.prepare` for context-dependent changes.

## 7. Hook points and payloads

“Result” below means `HookResult.payload`. `unchanged` does not need a payload.

| Hook | Kind | Timing | Input/result payload | skip / stop |
| --- | --- | --- | --- | --- |
| `agent.prepare` | Transform | After extension initialization | `AgentDraft -> AgentDraft` | Both make Agent initialization fail |
| `input.prepare` | Transform | Before user input is committed | `{text:string, delayed:bool}` | skip drops input; stop stops the run |
| `turn.prepare` | Transform | Before each model request is built | `AgentDraft -> AgentDraft`, with read-only context | skip skips the turn; stop exits the loop |
| `model.request.prepare` | Transform | Immediately before model I/O | Open Responses `CreateResponseBody` JSON | skip skips the turn; stop exits the loop |
| `model.stream.observe` | Observer | For each streaming event | Normalized event JSON | Not applicable |
| `model.response.prepare` | Transform | After the stream, before commit | `{text:string, items:Item[]}` | skip discards and retries next turn; stop exits |
| `tool.call.prepare` | Transform | Before the Action | `ToolCallRequest -> ToolCallRequest` | skip emits a skipped output; stop exits |
| `tools.call` | Action | Actual tool execution | `ToolCallRequest -> ToolResult` | Actions reject skip/stop |
| `tool.result.prepare` | Transform | Before committing tool output | `{call:ToolCallRequest,result:ToolResult}` | Current skip/stop retain the pre-Hook result |
| `context.commit` | Transform | Before any context write | `ContextCommitDraft -> ContextCommitDraft` | Does not commit; caller stops or skips its stage |
| `turn.complete` | Transform | At the end of a turn | `{iteration,called_tools,continue_loop}` | skip retains the original decision; stop exits |
| `agent.error` | Observer | When `run()` returns an error | `{error:string}` | Observer errors do not replace the original error |
| `agent.shutdown` | Observer | Before lifecycle shutdown | `{}` | Lifecycle shutdown still runs if the Observer fails |

### 7.1 model.stream.observe

Normalized events currently include:

```json
{"type":"text_delta","delta":"partial text"}
{"type":"output_item_added","item":{}}
{"type":"output_item_done","item":{}}
{"type":"error","message":"..."}
{"type":"other"}
```

This is a high-frequency Hook. Avoid expensive synchronous work.

### 7.2 ToolCallRequest and ToolResult

```json
{
  "call_id": "call_123",
  "tool_id": "tool-1",
  "name": "shell",
  "arguments": {"command":"pwd"}
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

A normal tool failure is still a successful WIT invocation with `success = false`. Return WIT `err(string)` only when the extension protocol itself fails.

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

Rust is the fully verified path in this repository. The easiest starting point is [`extensions/shell`](extensions/shell).

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

Build core Wasm and convert it to a Component:

```sh
cargo build --target wasm32-unknown-unknown --release
cargo run --manifest-path /path/to/ragent/Cargo.toml \
  --example componentize -- \
  target/wasm32-unknown-unknown/release/example.wasm \
  example.component.wasm
```

[`scripts/build-extensions.sh`](scripts/build-extensions.sh) implements this exact process.

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

ragent currently links only `ragent:extension/host`, not general WASI interfaces. A Go result that still imports `wasi:*` fails during loading. Inspect it with `wasm-tools component wit`. Until the host supports WASI, Go is an experimental path that requires resolving those imports yourself.

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

Because ragent does not provide WASI imports, use the ComponentizeJS Node API to disable `stdio`, `random`, `clocks`, `http`, and `fetch-event`, producing a pure component that depends only on the target WIT world.

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

Use `--stub-wasi`, or otherwise ensure that the Component has no WASI imports unsupported by the host. Python packages the interpreter with the application, so its Component is normally much larger and slower to load than a Rust extension.

## 12. Inspecting a Component

The final file must be a Component, not a core Wasm module:

```sh
wasm-tools validate extension.wasm --features component-model
wasm-tools component wit extension.wasm
```

The extracted world must be compatible with `ragent:extension/plugin@1.0.0`. The current host permits only `ragent:extension/host` from the project WIT. A result containing `wasi:*` or another import cannot currently be instantiated by ragent.

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
name = "example"
path = "extensions/example.wasm"
enabled = true

[extensions.config]
endpoint = "https://example.com"
mode = "strict"
```

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
- Avoid blocking work in the high-frequency `model.stream.observe` Hook.
- Test successful execution, invalid arguments, host errors, and non-zero exit status for every tool.
- Do not persist another extension's temporary tool ID. It is stable only within the current Agent process and draft chain.
- If `wasm-tools validate` succeeds but loading fails, inspect unsupported extra imports first.
