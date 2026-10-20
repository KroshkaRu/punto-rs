#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
data_dir=${XDG_DATA_HOME:-"$HOME/.local/share"}
binary_dir="$HOME/.local/bin"
binary="$binary_dir/punto-rs"
if [[ ${1:-} == --uninstall ]]; then
    rm -f -- "${XDG_CONFIG_HOME:-"$HOME/.config"}/autostart/punto-rs.desktop"
    rm -f -- "$binary" "$data_dir/ibus/component/punto-rs.xml" "$data_dir/applications/punto-rs.desktop" "$data_dir/icons/hicolor/scalable/apps/punto-rs.svg"
    if command -v gnome-extensions >/dev/null && command -v gdbus >/dev/null &&
       [[ $(gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.NameHasOwner org.gnome.Shell 2>/dev/null) == *true* ]]; then
        gnome-extensions disable punto-focus@punto-rs.local 2>/dev/null || true
    fi
    rm -rf -- "$data_dir/gnome-shell/extensions/punto-focus@punto-rs.local"
    rm -rf -- "$data_dir/licenses/punto-rs"
    printf '%s\n' 'Удалено. Настройки сохранены. Если использовался IBus, также уберите источник Punto RS из его настроек.'
    exit 0
fi
if [[ ${1:-} == --extension ]]; then
    version=$(gnome-shell --version | sed -E 's/[^0-9]*([0-9]+).*/\1/')
    extension="$data_dir/gnome-shell/extensions/punto-focus@punto-rs.local"
    mkdir -p -- "$extension"
    if (( version < 45 )); then format=legacy; else format=modern; fi
    cat "$root/gnome-extension/$format-prefix.js" "$root/gnome-extension/bridge.js" "$root/gnome-extension/$format-suffix.js" > "$extension/extension.js"
    install -m 644 -- "$root/gnome-extension/metadata.json" "$extension/metadata.json"
    printf '%s\n' 'Мост GNOME установлен. Выйдите и войдите в сеанс, затем:' 'gnome-extensions enable punto-focus@punto-rs.local'
    exit 0
fi
if [[ $# -gt 0 ]]; then printf '%s\n' 'Использование: scripts/install.sh [--extension|--uninstall]' >&2; exit 2; fi
command -v cargo >/dev/null || { printf '%s\n' 'Нужен Rust 1.88+: https://rustup.rs' >&2; exit 1; }
cargo build --manifest-path "$root/Cargo.toml" --release --locked
"$root/target/release/punto-rs" --check
mkdir -p -- "$binary_dir" "$data_dir/ibus/component" "$data_dir/applications" "$data_dir/icons/hicolor/scalable/apps"
install -m 755 -- "$root/target/release/punto-rs" "$binary"
# XML and desktop entries have different escaping rules; paths may contain spaces.
python3 - "$root" "$binary" "$data_dir" <<'PY'
import pathlib, sys
from xml.sax.saxutils import escape
root, binary, data = map(pathlib.Path, sys.argv[1:])
quoted = '"' + str(binary).replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$') + '"'
xml = (root / 'packaging/punto-rs.xml.in').read_text().replace('@BINARY@', escape(quoted))
(data / 'ibus/component/punto-rs.xml').write_text(xml)
desktop = (root / 'packaging/punto-rs.desktop.in').read_text().replace('@BINARY@', quoted.replace('%', '%%'))
(data / 'applications/punto-rs.desktop').write_text(desktop)
PY
install -m 644 -- "$root/packaging/punto-rs.svg" "$data_dir/icons/hicolor/scalable/apps/punto-rs.svg"
mkdir -p "$data_dir/licenses/punto-rs"
install -m 644 "$root/assets/material-icons/LICENSE" "$root/assets/material-icons/NOTICE" "$data_dir/licenses/punto-rs/"
if command -v update-desktop-database >/dev/null; then update-desktop-database "$data_dir/applications"; fi
if command -v gtk-update-icon-cache >/dev/null; then gtk-update-icon-cache -f -t "$data_dir/icons/hicolor" >/dev/null 2>&1 || true; fi
printf '%s\n' 'Установлено для текущего пользователя. Откройте Punto RS в меню приложений.' 'По умолчанию используется нативный ввод без смены системных раскладок и метода ввода.' 'IBus — добровольный режим. GNOME Wayland: см. README.md.'
