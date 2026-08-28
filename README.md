**English** | [简体中文](README.zh-CN.md)

# ragent

`ragent` is a minimal LLM agent built around WebAssembly Component extensions.

The core has only three responsibilities:

- non-streaming model request/response I/O;
- context commits;
- the Agent/ReAct loop.

Tools, System Prompt changes, model parameter changes, input processing, context processing, and lifecycle notifications are all implemented through WASM extensions. The core contains no hard-coded tool. The Shell and File Editor components in this repository are optional example extensions.

## How it works

```text
user input
  -> WASM hooks
  -> build model request
  -> non-streaming model I/O
  -> WASM hooks
  -> optional tool Action
  -> context commit
  -> next turn or finish
```

Extensions use a single WIT Component ABI and a JSON Hook protocol. For the architecture, every Hook contract, tool ownership rules, and development instructions for Rust, Go, AssemblyScript, and Python, see:

> [Extension development and Hook protocol](EXTENSIONS.md)

## Requirements

- Rust toolchain
- The `wasm32-wasip2` target when building extensions
- A model service compatible with the OpenAI Responses API

Install the Rust WASM target:

```sh
rustup target add wasm32-wasip2
```

## Build

Build the Agent:

```sh
cargo build --release
```

Build the bundled extensions (`shell`, `file_editor`, and `image_viewer`):

```sh
# Build all extensions (default)
./scripts/build-extensions.sh

# Or build specific extensions (comma-separated list)
./scripts/build-extensions.sh shell
./scripts/build-extensions.sh shell,file_editor,image_viewer
```

The generated Components are written to:

```text
extensions/shell/target/wasm32-wasip2/release/ragent_shell_extension.wasm
extensions/file_editor/target/wasm32-wasip2/release/ragent_file_editor_extension.wasm
extensions/image_viewer/target/wasm32-wasip2/release/ragent_image_viewer_extension.wasm
```

Build and install or update them in the active ragent configuration directory:

```sh
# Install all extensions (default)
./scripts/install-extensions.sh

# Or install specific extensions
./scripts/install-extensions.sh shell
./scripts/install-extensions.sh file_editor
```

The install/update script follows `XDG_CONFIG_HOME` when set, otherwise it uses
`~/.config/ragent`. It atomically replaces installed extensions (`extensions/shell.wasm`, `extensions/file_editor.wasm`, `extensions/image_viewer.wasm`). If `config.toml` does not exist, it creates the default configuration; if `config.toml` already exists, it checks and appends missing extension entries to the end of the file without duplicating existing configurations.

## Configuration

### Environment Variables

Configure the API connection with environment variables:

```sh
export ROSETTA_URL="https://example.com/v1"
export ROSETTA_TOKEN="your-token"
```

> Note: Only `ROSETTA_URL` and `ROSETTA_TOKEN` are supported from environment variables. Model parameters and reasoning options are configured via `config.toml`.

### Global Configuration File (`~/.config/ragent/config.toml`)

`config.toml` configures global model parameters (including reasoning / thinking effort) and manages extensions.

Example configuration:

```toml
[model]
name = "gemini-3.7-flash"
temperature = 0.7
max_output_tokens = 8192

# Reasoning / Thinking configuration (optional)
[model.reasoning]
effort = "high"      # none | low | medium | high | xhigh
summary = "concise"  # auto | concise | detailed

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

Relative paths for extensions are resolved from `~/.config/ragent/`.
The Shell timeout defaults to 1,800 seconds (30 minutes) when omitted. Set it to
`0` to disable the timeout. Each Shell tool call may override it with the optional
`timeout_seconds` argument; `0` also disables the timeout for that call.

### Project Configuration File (`.ragent/config.toml`)

A project-specific incremental configuration can be placed at `.ragent/config.toml` in the current working directory:

- **Merging Rules**: Sub-node keys (leaf keys) recursively override values from the global configuration; unconfigured keys fall back to global settings.
- **Extension Restrictions**:
  - Only `name`, `enabled`, and `config` fields are permitted in project extension entries (extra fields such as `path` are rejected).
  - Extension `name`s not present in global configuration are not allowed.
  - Extension execution order strictly follows the global configuration and is not affected by the ordering in project config.
  - Duplicate extension names are disallowed in both global and project configurations.

Project configuration example:

```toml
[model]
temperature = 0.2

[model.reasoning]
effort = "medium"

[[extensions]]
name = "file_editor"
enabled = false

[[extensions]]
name = "shell"
[extensions.config]
default_timeout_seconds = 300
```

## Usage

Start a session:

```sh
cargo run -- "List the files in the current directory"
```

Specify / override model name:

```sh
cargo run -- "Write a quicksort implementation" -m gemini-2.5-pro
cargo run -- -m gpt-4o "Analyze this project"
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
cargo run -- s sess_example "Continue with another model" -m claude-3-5-sonnet
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

The extension tests cover:
- Shell: successful commands, non-zero exit codes, configured timeouts, per-call overrides, and timeout disabling.
- File Editor: file creation and full overwriting (`write_file`), unique search-and-replace (`replace_in_file`), non-unique match rejections, and failure error handling.

## Repository layout

```text
src/agent.rs              Agent I/O and loop
src/context.rs            context storage and commits
src/wasm/                 WASM loading, dispatch, and protocol types
wit/ragent-extension.wit  Component ABI
extensions/               optional bundled extension examples (shell, file_editor, image_viewer)
EXTENSIONS.md             extension development and Hook protocol
```
