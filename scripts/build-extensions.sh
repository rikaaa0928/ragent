#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
guest_wasm="$project_dir/extensions/shell/target/wasm32-unknown-unknown/release/ragent_shell_extension.wasm"
component_wasm="$project_dir/extensions/shell/target/component/ragent_shell_extension.wasm"

cargo build \
    --manifest-path "$project_dir/extensions/shell/Cargo.toml" \
    --target wasm32-unknown-unknown \
    --release

cargo run \
    --manifest-path "$project_dir/tools/componentize/Cargo.toml" \
    -- "$guest_wasm" "$component_wasm"

echo "$component_wasm"
