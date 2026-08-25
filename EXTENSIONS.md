# ragent WASM extensions

The core owns only model I/O, context commits, and the execution loop. Optional
behavior is implemented by WebAssembly Components using
[`wit/ragent-extension.wit`](wit/ragent-extension.wit).

The Component ABI intentionally stays small: `metadata`, `initialize`, one
JSON-based `invoke`, and `shutdown`. New hook points only add JSON contracts;
they do not require a new WIT world.

## Dispatch types

Hook point and dispatch type are separate concepts:

- `transform`: runs serially by ascending priority. Each extension receives the
  previous validated value and may add, update, delete, or reorder fields.
- `action`: routes a side effect to one explicit owner. Tool execution uses this
  type and is never broadcast.
- `observer`: broadcasts a notification concurrently. Its returned value is
  ignored.

There is no separate provider or gate type. A transform starting from an empty
list covers contribution, and `reject` covers gating.

Every transform/action returns this envelope:

```json
{
  "action": "continue",
  "payload": {},
  "reason": null
}
```

Actions are `continue`, `unchanged`, `reject`, `skip`, and `stop`. `continue`
requires `payload`; `reject` should provide `reason`. `skip` skips the current
stage and `stop` terminates the surrounding flow where that hook permits it.

The request envelope contains `hook`, `protocol_version`, `invocation_id`, an
optional `iteration`, and `payload`.

## Validation and failure policy

Transform output is validated immediately after every extension. The next
extension never sees an invalid intermediate value.

- `failure = "abort"`: fail with the extension ID and hook name.
- `failure = "ignore"`: discard that extension's output and resume from the
  value immediately before it ran.

Core checks include model parameter ranges, tool schema shape, unique tool IDs
and names, stable tool ownership, request/response shapes, and context protocol
relationships. Lower numeric priorities run first; equal priorities retain
configuration order.

Tools are carried as `ToolEntry` values. A tool newly appended by an extension
must omit `id` and `owner`. The core assigns both. Later transforms may edit,
disable, remove, or reorder that tool, but cannot take over its ownership:

```json
{
  "id": "tool-1",
  "owner": "shell",
  "enabled": true,
  "name": "shell",
  "description": "...",
  "parameters": { "type": "object" }
}
```

An extension adding a tool must also own the `tools.call` action.

## Agent flow and hook points

```text
load config -> metadata -> initialize
  -> agent.prepare (Transform AgentDraft)
  -> immutable BaseAgentState

input
  -> input.prepare (Transform)
  -> context.commit (Transform)

each turn
  -> clone BaseAgentState + current context
  -> turn.prepare (Transform AgentDraft)
  -> model.request.prepare (Transform)
  -> model I/O
       -> model.stream.observe (Observer, per normalized event)
  -> model.response.prepare (Transform)
  -> context.commit (Transform)
  -> if tool calls:
       tool.call.prepare (Transform)
       -> tools.call (Action, routed by immutable owner)
       -> tool.result.prepare (Transform)
       -> context.commit (Transform)
  -> turn.complete (Transform)
  -> continue or finish

any run error -> agent.error (Observer)
shutdown      -> agent.shutdown (Observer) -> lifecycle shutdown
```

`agent.prepare` receives the base system prompt
`你是一个高效、精准的 AI 智能体助手`, model parameters, and an empty tool
list. `turn.prepare` receives a clone of that validated base draft plus a
read-only current-context view, so tools, system prompt, and model parameters can
be changed dynamically without mutating the next turn's base state.

## Bootstrap configuration

The bootstrap file is `~/.config/ragent/config.toml`. Its only responsibility is
locating extensions and supplying initialization configuration:

```toml
[[extensions]]
name = "shell"
path = "extensions/shell.wasm"
enabled = true

[extensions.config]
example = "value"
```

Relative component paths are resolved from the configuration directory.

## Build the bundled Shell extension

```sh
./scripts/build-extensions.sh
```

The component is written to
`extensions/shell/target/component/ragent_shell_extension.wasm`. Shell registers
its tool by appending it during `agent.prepare`, then owns execution through the
`tools.call` action.
