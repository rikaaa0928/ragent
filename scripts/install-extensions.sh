#!/bin/sh
set -eu

# 支持通过参数传入以逗号分隔的扩展名列表，如: ./scripts/install-extensions.sh shell
# 默认安装所有扩展
if [ "$#" -gt 1 ]; then
    echo "usage: $0 [shell,file_editor,image_viewer]" >&2
    exit 2
fi

filter=$(printf '%s' "${1:-}" | tr -d '[:space:]')
if [ "$#" -eq 1 ] && [ -z "$filter" ]; then
    echo "Error: extension filter must not be empty" >&2
    exit 2
fi

ALL_EXTENSIONS="shell,file_editor,image_viewer"

is_selected() {
    ext_name="$1"
    if [ -z "$filter" ]; then
        return 0
    fi
    case ",$filter," in
        *,"$ext_name",*) return 0 ;;
        *) return 1 ;;
    esac
}

# 校验传入的扩展名是否存在
if [ -n "$filter" ]; then
    old_ifs="$IFS"
    IFS=','
    for ext in $filter; do
        case ",$ALL_EXTENSIONS," in
            *,"$ext",*) ;;
            *)
                echo "Error: unknown extension '$ext'. Available extensions: $ALL_EXTENSIONS" >&2
                exit 1
                ;;
        esac
    done
    IFS="$old_ifs"
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

extensions_dir="$config_dir/extensions"
config_file="$config_dir/config.toml"
temp_component=""
temp_config=""

cleanup() {
    if [ -n "$temp_component" ]; then
        rm -f "$temp_component"
    fi
    if [ -n "$temp_config" ]; then
        rm -f "$temp_config"
    fi
}
trap cleanup EXIT HUP INT TERM

has_extension_config() {
    extension_name="$1"
    awk -v wanted="$extension_name" '
        function trim(value) {
            sub(/^[[:space:]]+/, "", value)
            sub(/[[:space:]]+$/, "", value)
            return value
        }
        {
            line = $0
            sub(/^[[:space:]]+/, "", line)
            if (line ~ /^\[\[extensions\]\][[:space:]]*(#.*)?$/) {
                in_extension = 1
                next
            }
            if (line ~ /^\[/) {
                in_extension = 0
                next
            }
            if (in_extension && line ~ /^name[[:space:]]*=/) {
                sub(/^name[[:space:]]*=[[:space:]]*/, "", line)
                sub(/[[:space:]]*(#.*)?$/, "", line)
                double_quoted = "\"" wanted "\""
                single_quoted = sprintf("%c%s%c", 39, wanted, 39)
                if (trim(line) == double_quoted || trim(line) == single_quoted) {
                    found = 1
                }
            }
        }
        END { exit found ? 0 : 1 }
    ' "$config_file"
}

append_extension_config() {
    extension_name="$1"
    temp_config=$(mktemp "$config_dir/.config.toml.XXXXXX")
    if [ -e "$config_file" ]; then
        cp -p "$config_file" "$temp_config"
        printf '\n' >> "$temp_config"
    else
        chmod 0644 "$temp_config"
    fi

    {
        printf '%s\n' '[[extensions]]'
        printf 'name = "%s"\n' "$extension_name"
        printf 'path = "extensions/%s.wasm"\n' "$extension_name"
        printf '%s\n' 'enabled = true'
        if [ "$extension_name" = "shell" ]; then
            printf '\n'
            printf '%s\n' '[extensions.config]'
            printf '%s\n' 'default_timeout_seconds = 1800'
        fi
    } >> "$temp_config"

    mv -f "$temp_config" "$config_file"
    temp_config=""
}

ensure_extension_config() {
    extension_name="$1"
    if [ -e "$config_file" ] && has_extension_config "$extension_name"; then
        echo "config for '$extension_name' already exists in $config_file"
        return
    fi

    if [ -e "$config_file" ]; then
        echo "appending '$extension_name' config to $config_file"
    else
        echo "creating $config_file with '$extension_name' config"
    fi
    append_extension_config "$extension_name"
}

install_extension() {
    extension_name="$1"
    component_wasm="$2"
    installed_wasm="$extensions_dir/$extension_name.wasm"
    temp_component=$(mktemp "$extensions_dir/.$extension_name.wasm.XXXXXX")
    cp "$component_wasm" "$temp_component"
    chmod 0644 "$temp_component"
    mv -f "$temp_component" "$installed_wasm"
    temp_component=""
    echo "installed $installed_wasm"
    ensure_extension_config "$extension_name"
}

# 先调用编译脚本编译指定的扩展
if [ -n "$filter" ]; then
    "$project_dir/scripts/build-extensions.sh" "$filter"
else
    "$project_dir/scripts/build-extensions.sh"
fi

mkdir -p "$extensions_dir"

if is_selected "shell"; then
    install_extension \
        "shell" \
        "$project_dir/extensions/shell/target/wasm32-wasip2/release/ragent_shell_extension.wasm"
fi

if is_selected "file_editor"; then
    install_extension \
        "file_editor" \
        "$project_dir/extensions/file_editor/target/wasm32-wasip2/release/ragent_file_editor_extension.wasm"
fi

if is_selected "image_viewer"; then
    install_extension \
        "image_viewer" \
        "$project_dir/extensions/image_viewer/target/wasm32-wasip2/release/ragent_image_viewer_extension.wasm"
fi
