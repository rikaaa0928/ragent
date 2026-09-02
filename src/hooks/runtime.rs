use crate::error::AgentError;
use crate::hooks::protocol::{ExtensionMetadata, HookRequest};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::Mutex;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{
    DirPerms, FilePerms, ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView,
};

wasmtime::component::bindgen!({
    path: "wit",
    world: "plugin",
    imports: { default: async },
    exports: { default: async },
});

struct HostState {
    wasi_ctx: WasiCtx,
    table: ResourceTable,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.table,
        }
    }
}

impl ragent::extension::host::Host for HostState {
    async fn execute_command_with_timeout(
        &mut self,
        command: String,
        timeout_ms: u64,
    ) -> ragent::extension::host::CommandOutput {
        run_command(command, timeout_ms).await
    }
}

async fn run_command(command: String, timeout_ms: u64) -> ragent::extension::host::CommandOutput {
    let mut child = tokio::process::Command::new("sh");
    child.arg("-c").arg(command).kill_on_drop(true);
    let output = if timeout_ms == 0 {
        child.output().await
    } else {
        match tokio::time::timeout(Duration::from_millis(timeout_ms), child.output()).await {
            Ok(result) => result,
            Err(_) => return command_error(format!("command timed out after {timeout_ms} ms")),
        }
    };
    match output {
        Ok(output) => ragent::extension::host::CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            error: None,
        },
        Err(error) => command_error(error),
    }
}

fn command_error(error: impl ToString) -> ragent::extension::host::CommandOutput {
    ragent::extension::host::CommandOutput {
        exit_code: -1,
        stdout: String::new(),
        stderr: String::new(),
        error: Some(error.to_string()),
    }
}

struct PluginRuntime {
    store: Store<HostState>,
    bindings: Plugin,
}

pub struct WasmPlugin {
    pub name: String,
    metadata: ExtensionMetadata,
    runtime: Mutex<PluginRuntime>,
    #[cfg(any(test, debug_assertions, feature = "test-utils"))]
    shutdown_call_count: std::sync::atomic::AtomicU64,
    #[cfg(any(test, debug_assertions, feature = "test-utils"))]
    simulate_shutdown_failure: std::sync::atomic::AtomicBool,
}

impl WasmPlugin {
    pub async fn load_from_file(name: impl Into<String>, path: &Path) -> Result<Self, AgentError> {
        Self::load_from_file_with_dirs(name, path, &[]).await
    }

    pub async fn load_from_file_with_dirs(
        name: impl Into<String>,
        path: &Path,
        preopen_dirs: &[PathBuf],
    ) -> Result<Self, AgentError> {
        let mut config = Config::new();
        config.async_support(true);
        let engine = Engine::new(&config).map_err(tool_error)?;
        let component = Component::from_file(&engine, path).map_err(|error| {
            AgentError::ToolError(format!(
                "failed to load WebAssembly component from {path:?}: {error}"
            ))
        })?;

        let mut linker = Linker::new(&engine);
        // Link WASI 0.2 (P2)
        wasmtime_wasi::p2::add_to_linker_async(&mut linker).map_err(tool_error)?;
        // Link project lifecycle & host interfaces
        Plugin::add_to_linker::<_, wasmtime::component::HasSelf<_>>(&mut linker, |state| state)
            .map_err(tool_error)?;

        let mut wasi_builder = WasiCtxBuilder::new();
        wasi_builder.inherit_stdout().inherit_stderr();

        if preopen_dirs.is_empty() {
            // Default preopen current directory
            if let Err(e) = wasi_builder.preopened_dir(".", ".", DirPerms::all(), FilePerms::all())
            {
                eprintln!("Warning: failed to preopen current directory: {e}");
            }
        } else {
            for dir in preopen_dirs {
                if let Some(dir_str) = dir.to_str() {
                    let _ = wasi_builder.preopened_dir(
                        dir_str,
                        dir_str,
                        DirPerms::all(),
                        FilePerms::all(),
                    );
                }
            }
            let _ = wasi_builder.preopened_dir(".", ".", DirPerms::all(), FilePerms::all());
        }

        let host_state = HostState {
            wasi_ctx: wasi_builder.build(),
            table: ResourceTable::new(),
        };

        let mut store = Store::new(&engine, host_state);
        let bindings = Plugin::instantiate_async(&mut store, &component, &linker)
            .await
            .map_err(tool_error)?;
        let metadata_json = bindings
            .ragent_extension_lifecycle()
            .call_metadata(&mut store)
            .await
            .map_err(tool_error)?;
        let metadata: ExtensionMetadata =
            serde_json::from_str(&metadata_json).map_err(|error| {
                AgentError::ToolError(format!("extension metadata is invalid: {error}"))
            })?;

        Ok(Self {
            name: name.into(),
            metadata,
            runtime: Mutex::new(PluginRuntime { store, bindings }),
            #[cfg(any(test, debug_assertions, feature = "test-utils"))]
            shutdown_call_count: std::sync::atomic::AtomicU64::new(0),
            #[cfg(any(test, debug_assertions, feature = "test-utils"))]
            simulate_shutdown_failure: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub fn metadata(&self) -> &ExtensionMetadata {
        &self.metadata
    }

    pub async fn initialize(&self, config: &serde_json::Value) -> Result<(), AgentError> {
        let config = serde_json::to_string(config)?;
        let mut runtime = self.runtime.lock().await;
        let PluginRuntime { store, bindings } = &mut *runtime;
        bindings
            .ragent_extension_lifecycle()
            .call_initialize(store, &config)
            .await
            .map_err(tool_error)?
            .map_err(|error| AgentError::ToolError(format!("{}: {error}", self.name)))
    }

    pub async fn invoke(&self, request: &HookRequest) -> Result<serde_json::Value, AgentError> {
        let request = serde_json::to_string(request)?;
        let mut runtime = self.runtime.lock().await;
        let PluginRuntime { store, bindings } = &mut *runtime;
        let response = bindings
            .ragent_extension_lifecycle()
            .call_invoke(store, &request)
            .await
            .map_err(tool_error)?
            .map_err(|error| AgentError::ToolError(format!("{}: {error}", self.name)))?;
        serde_json::from_str(&response).map_err(AgentError::JsonError)
    }

    #[cfg(any(test, debug_assertions, feature = "test-utils"))]
    pub fn set_simulate_shutdown_failure(&self, fail: bool) {
        self.simulate_shutdown_failure
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(any(test, debug_assertions, feature = "test-utils"))]
    pub fn shutdown_call_count(&self) -> u64 {
        self.shutdown_call_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn shutdown(&self) -> Result<(), AgentError> {
        #[cfg(any(test, debug_assertions, feature = "test-utils"))]
        {
            self.shutdown_call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self
                .simulate_shutdown_failure
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(AgentError::ToolError(format!(
                    "{}: injected shutdown failure",
                    self.name
                )));
            }
        }

        let mut runtime = self.runtime.lock().await;
        let PluginRuntime { store, bindings } = &mut *runtime;
        bindings
            .ragent_extension_lifecycle()
            .call_shutdown(store)
            .await
            .map_err(tool_error)
    }
}

fn tool_error(error: impl std::fmt::Display) -> AgentError {
    AgentError::ToolError(error.to_string())
}
