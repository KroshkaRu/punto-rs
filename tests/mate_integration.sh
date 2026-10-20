#!/usr/bin/env bash
# Only in a disposable Xvfb + dbus-run-session, never on the user's desktop.
set -euo pipefail
export XDG_CURRENT_DESKTOP=MATE DESKTOP_SESSION=mate
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/tmp/punto-mate-runtime}
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
gsettings set org.mate.peripherals-keyboard-xkb.kbd layouts "['us', 'ru']"
gsettings set org.mate.peripherals-keyboard-xkb.kbd options "['grp\tgrp:ctrl_shift_toggle']"
gsettings set org.mate.peripherals-keyboard-xkb.general group-per-window true
marco --sm-disable >/tmp/punto-marco.log 2>&1 &
marco_pid=$!
mate-settings-daemon --no-daemon >/tmp/punto-msd.log 2>&1 &
settings_pid=$!
trap 'kill "$settings_pid" "$marco_pid" 2>/dev/null || true' EXIT
sleep 1
python3 tests/gtk4_integration.py "$1"
