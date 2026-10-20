#!/usr/bin/env bash
set -euo pipefail
project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
test_dir=$(mktemp -d /tmp/punto-install.XXXXXX)
trap 'rm -rf -- "$test_dir"' EXIT
export CARGO_HOME=${CARGO_HOME:-"$HOME/.cargo"}
export RUSTUP_HOME=${RUSTUP_HOME:-"$HOME/.rustup"}
export PATH="$CARGO_HOME/bin:$PATH"
export XDG_DATA_HOME="$test_dir/data space"
export XDG_CONFIG_HOME="$test_dir/config"
test_home="$test_dir/user home"
mkdir -p "$test_home" "$XDG_CONFIG_HOME/punto-rs"
env HOME="$test_home" "$project/scripts/install.sh"
env HOME="$test_home" "$project/scripts/install.sh" --extension
python3 - "$XDG_DATA_HOME" "$test_home" <<'PY'
import pathlib, sys, xml.etree.ElementTree as ET
data, home = map(pathlib.Path, sys.argv[1:])
binary = home / '.local/bin/punto-rs'
assert binary.is_file()
component = ET.parse(data / 'ibus/component/punto-rs.xml')
assert component.find('exec').text == f'"{binary}" --ibus'
assert (data / 'applications/punto-rs.desktop').is_file()
assert (data / 'gnome-shell/extensions/punto-focus@punto-rs.local/extension.js').is_file()
PY
printf '%s\n' 'preserve user settings' > "$XDG_CONFIG_HOME/punto-rs/config.toml"
mkdir -p "$XDG_CONFIG_HOME/autostart"
printf '%s\n' '[Desktop Entry]' > "$XDG_CONFIG_HOME/autostart/punto-rs.desktop"
env HOME="$test_home" "$project/scripts/install.sh" --uninstall
test ! -e "$test_home/.local/bin/punto-rs"
test ! -e "$XDG_DATA_HOME/ibus/component/punto-rs.xml"
test -f "$XDG_CONFIG_HOME/punto-rs/config.toml"
test ! -e "$XDG_CONFIG_HOME/autostart/punto-rs.desktop"
printf '%s\n' 'Install/uninstall: paths with spaces and settings preservation OK'
