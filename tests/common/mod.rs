use std::path::PathBuf;

#[allow(dead_code)]
pub fn shell_component_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("extensions/shell/target/wasm32-wasip2/release/ragent_shell_extension.wasm")
}

#[allow(dead_code)]
pub fn file_editor_component_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "extensions/file_editor/target/wasm32-wasip2/release/ragent_file_editor_extension.wasm",
    )
}

#[allow(dead_code)]
pub fn image_viewer_component_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "extensions/image_viewer/target/wasm32-wasip2/release/ragent_image_viewer_extension.wasm",
    )
}
