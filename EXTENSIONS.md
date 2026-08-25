# ragent extensions

The agent core owns model I/O, context progression, and the execution loop. All
optional behavior is supplied by WebAssembly Components implementing
[`wit/ragent-extension.wit`](wit/ragent-extension.wit).

An extension exposes four lifecycle operations:

- `metadata`: identifies the extension and declares hook subscriptions.
- `initialize`: receives its bootstrap configuration once.
- `invoke`: handles a versioned hook request encoded as JSON.
- `shutdown`: releases extension state.

Hook subscriptions use one of five execution semantics:

- `provider`: contributes values, such as tools.
- `transform`: modifies a value in deterministic priority order.
- `gate`: accepts or rejects an operation.
- `action`: owns an operation, such as a tool call.
- `observer`: receives a notification without modifying the flow.

Lower numeric priorities run first. Duplicate extension IDs and duplicate tool
names are rejected instead of being overwritten.

## Bootstrap configuration

The bootstrap file is `~/.config/ragent/config.toml` by default. Its only job is
to locate extensions and pass their initial configuration:

```toml
[[extensions]]
name = "shell"
path = "extensions/shell.wasm"
enabled = true

[extensions.config]
example = "value"
```

Relative component paths are resolved from the directory containing the
bootstrap configuration.

## Build the example extension

Run:

```sh
./scripts/build-extensions.sh
```

The generated component is written to:

```text
extensions/shell/target/component/ragent_shell_extension.wasm
```

The build uses the installed `wasm32-unknown-unknown` Rust target and a
project-local component encoder; it does not require `cargo-component` or a
globally installed `wasm-tools` command.

## Defined core hooks

- `config.resolve` (`transform`)
- `loop.before` (`observer`)
- `context.prepare` (`transform`)
- `tools.list` (`provider`)
- `model.request.transform` (`transform`)
- `model.response` (`observer`)
- `tools.call` (`action`)
- `tool.result.transform` (`transform`)
- `loop.after` (`observer`)

Hook payloads carry `version = 1`. Extensions should reject versions they do
not understand rather than guessing their shape.
