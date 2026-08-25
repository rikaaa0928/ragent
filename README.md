**English** | [简体中文](README.zh-CN.md)

# ragent

`ragent` is a minimal streaming LLM agent built around WebAssembly Component extensions.

The core has only three responsibilities:

- model request and streaming response I/O;
- context commits;
- the Agent/ReAct loop.

Tools, System Prompt changes, model parameter changes, input processing, context processing, and lifecycle notifications are all implemented through WASM extensions. The core contains no hard-coded tool. The Shell component in this repository is an optional example extension.

## How it works

```text
user input
  -> WASM hooks
  -> build model request
  -> streaming model I/O
  -> WASM hooks
  -> optional tool Action
  -> context commit
  -> next turn or finish
```

Extensions use a single WIT Component ABI and a JSON Hook protocol. For the architecture, every Hook contract, tool ownership rules, and development instructions for Rust, Go, AssemblyScript, and Python, see:

> [Extension development and Hook protocol](EXTENSIONS.md)

## Requirements

- Rust toolchain
- The `wasm32-unknown-unknown` target when building the example extension
- A model service compatible with the OpenAI Responses API

Install the Rust WASM target:

```sh
rustup target add wasm32-unknown-unknown
```

## Build

Build the Agent:

```sh
cargo build --release
```

Build the bundled Shell example extension:

```sh
./scripts/build-extensions.sh
```

The generated Component is written to:

```text
extensions/shell/target/component/ragent_shell_extension.wasm
```

## Configuration

Configure the model connection with environment variables:

```sh
export ROSETTA_URL="https://example.com/v1"
export ROSETTA_TOKEN="your-token"
export MODEL_NAME="your-model"
```

Extensions are loaded from `~/.config/ragent/config.toml`. Install the example Shell extension:

```sh
mkdir -p ~/.config/ragent/extensions
cp extensions/shell/target/component/ragent_shell_extension.wasm \
  ~/.config/ragent/extensions/shell.wasm
```

Then create the configuration:

```toml
[[extensions]]
name = "shell"
path = "extensions/shell.wasm"
enabled = true

[extensions.config]
default_timeout_seconds = 1800
```

The configuration file currently only discovers extensions and supplies their initialization configuration. Relative paths are resolved from `~/.config/ragent/`.
The Shell timeout defaults to 1,800 seconds (30 minutes) when omitted. Set it to
`0` to disable the timeout. Each Shell tool call may override it with the optional
`timeout_seconds` argument; `0` also disables the timeout for that call.

## Usage

Start a session:

```sh
cargo run -- "List the files in the current directory"
```

Use a custom session directory:

```sh
cargo run -- "Analyze this project" -d .ragent/sessions
```

Manage sessions:

```sh
cargo run -- s list
cargo run -- s view sess_example
cargo run -- s sess_example "Continue the analysis"
cargo run -- s del sess_example
```

Show all commands:

```sh
cargo run -- --help
```

Press `Ctrl+C` once during a run to cancel the current model, Hook, or tool work,
run extension shutdown, and save the session. Press it again while cleanup is in
progress to exit immediately with status 130.

## Default behavior

- The default System Prompt is exactly: `你是一个高效、精准的 AI 智能体助手`
- The tool list is empty when no extension is loaded.
- Context is never pruned automatically.
- `max_iterations = 0` means that the loop is unlimited.
- Session IDs may contain only letters, digits, `_`, and `-`, with a maximum length of 64 characters.
- `AgentSender::cancel()` exposes the same cancellation path for embedded callers.

## Verification

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

The Shell extension tests cover successful commands, non-zero exit codes,
configured timeouts, per-call overrides, and timeout disabling. A non-zero exit
code or timeout is returned to the Agent as a failed tool result.

## Repository layout

```text
src/agent.rs              Agent I/O and loop
src/context.rs            context storage and commits
src/wasm/                 WASM loading, dispatch, and protocol types
wit/ragent-extension.wit  Component ABI
extensions/               optional bundled extension examples
tools/componentize/       generic Core Wasm to Component converter
EXTENSIONS.md             extension development and Hook protocol
```
