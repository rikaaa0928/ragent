#!/bin/sh
set -eu

if [ "$#" -ne 0 ]; then
    echo "usage: $0" >&2
    exit 2
fi

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

if [ -n "${XDG_CONFIG_HOME:-}" ]; then
    config_dir="$XDG_CONFIG_HOME/ragent"
elif [ -n "${HOME:-}" ]; then
    config_dir="$HOME/.config/ragent"
else
    echo "cannot determine ragent config directory: HOME and XDG_CONFIG_HOME are unset" >&2
    exit 1
fi

component_wasm="$project_dir/extensions/shell/target/component/ragent_shell_extension.wasm"
extensions_dir="$config_dir/extensions"
installed_wasm="$extensions_dir/shell.wasm"
config_file="$config_dir/config.toml"
temp_component=""
temp_config=""
install_action="installed"

cleanup() {
    if [ -n "$temp_component" ]; then
        rm -f "$temp_component"
    fi
    if [ -n "$temp_config" ]; then
        rm -f "$temp_config"
    fi
}
trap cleanup EXIT HUP INT TERM

"$project_dir/scripts/build-extensions.sh"

mkdir -p "$extensions_dir"
if [ -e "$installed_wasm" ]; then
    install_action="updated"
fi
temp_component=$(mktemp "$extensions_dir/.shell.wasm.XXXXXX")
cp "$component_wasm" "$temp_component"
chmod 0644 "$temp_component"
mv -f "$temp_component" "$installed_wasm"
temp_component=""

if [ ! -e "$config_file" ]; then
    temp_config=$(mktemp "$config_dir/.config.toml.XXXXXX")
    {
        printf '%s\n' '[[extensions]]'
        printf '%s\n' 'name = "shell"'
        printf '%s\n' 'path = "extensions/shell.wasm"'
        printf '%s\n' 'enabled = true'
        printf '\n'
        printf '%s\n' '[extensions.config]'
        printf '%s\n' 'default_timeout_seconds = 1800'
    } > "$temp_config"
    chmod 0644 "$temp_config"
    mv "$temp_config" "$config_file"
    temp_config=""
    echo "created $config_file"
else
    echo "kept existing $config_file unchanged"
    echo "ensure it contains an enabled shell entry pointing to extensions/shell.wasm"
fi

echo "$install_action $installed_wasm"
