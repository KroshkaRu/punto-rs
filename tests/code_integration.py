#!/usr/bin/env python3
"""Actual VS Code, offline temporary file/profile, only inside Xvfb + dbus-run-session."""
import ctypes
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
from gi.repository import Gio, GLib


def wait(check, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    assert check(), 'VS Code integration timed out'


with tempfile.TemporaryDirectory(prefix='punto-code-') as directory:
    os.environ.update(HOME=directory, XDG_CONFIG_HOME=directory, XDG_SESSION_TYPE='x11',
                      GTK_IM_MODULE='xim', XMODIFIERS='@im=none', NO_AT_BRIDGE='0')
    root = pathlib.Path(directory)
    config = root / 'punto-rs/config.toml'
    config.parent.mkdir()
    config.write_text('backend = "native"\nautomatic = false\nremember_layout = false\n')
    profile = root / 'profile/User'
    profile.mkdir(parents=True)
    (profile / 'settings.json').write_text(json.dumps({
        'workbench.startupEditor': 'none', 'window.title': 'Punto Code Test',
        'security.workspace.trust.enabled': False, 'update.mode': 'none',
        'chat.disableAIFeatures': True,
        'telemetry.telemetryLevel': 'off', 'extensions.autoCheckUpdates': False,
        'editor.accessibilitySupport': 'off', 'editor.quickSuggestions': False,
        'files.autoSave': 'afterDelay', 'files.autoSaveDelay': 100,
    }))
    document = root / 'word.txt'
    document.write_text('')
    xlib = ctypes.CDLL('libX11.so.6')
    xlib.XOpenDisplay.restype = ctypes.c_void_p
    display = xlib.XOpenDisplay(None)
    assert display  # Keep Xvfb connected so it cannot reset the keymap between clients.
    subprocess.run(['setxkbmap', '-layout', 'us,ru', '-option', '',
                    '-option', 'grp:ctrl_shift_toggle'], check=True)
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def call(method, args=None):
        return bus.call_sync('org.punto.Switcher', '/org/punto/Switcher',
                             'org.punto.Switcher', method, args, None,
                             Gio.DBusCallFlags.NONE, 2000, None).unpack()

    def ready():
        try:
            return call('InputReady')[0]
        except GLib.Error:
            return False

    def keys(*chords):
        subprocess.run(['xdotool', 'key', '--delay', '30', *chords], check=True)

    def physical(value):
        keys(*['space' if char == ' ' else char for char in value])

    def reset():
        keys('ctrl+a', 'BackSpace')
        wait(lambda: document.read_text() == '')
        call('SetLayout', GLib.Variant('(s)', ('EN',)))

    daemon = subprocess.Popen([sys.argv[1], '--daemon'])
    editor = subprocess.Popen([os.environ['PUNTO_TEST_CODE'], '--no-sandbox', '--disable-gpu',
                               '--skip-welcome', '--skip-release-notes',
                               '--ozone-platform=x11', '--disable-extensions',
                               '--user-data-dir=' + str(root / 'profile'), str(document)],
                              stderr=open(root / 'code.log', 'w'))
    try:
        wait(ready)
        window = []

        def find_window():
            window[:] = subprocess.run(['xdotool', 'search', '--name', '^Punto Code Test$'],
                                       capture_output=True, text=True).stdout.splitlines()
            return bool(window)

        wait(find_window)
        subprocess.run(['xdotool', 'windowfocus', '--sync', window[-1]], check=True)
        keys('Escape', 'ctrl+1')
        call('SetLayout', GLib.Variant('(s)', ('EN',)))
        for tail in (' ', '  '):
            physical('ghbdtn' + tail)
            wait(lambda: document.read_text() == 'ghbdtn' + tail)
            keys('Pause')
            wait(lambda: document.read_text() == 'привет' + tail)
            keys('ctrl+BackSpace')
            wait(lambda: document.read_text() == 'ghbdtn' + tail)
            reset()
        print('VS Code: Pause and Ctrl+Backspace after one/two spaces OK', flush=True)
        config.write_text('backend = "native"\nremember_layout = false\n')
        call('Reload')
        for original, expected in [('ghbdtn ', 'привет '), ('j ', 'о ')]:
            print('VS Code automatic:', repr(original), '→', repr(expected), flush=True)
            physical(original)
            wait(lambda: document.read_text() == expected)
            assert call('Status')[0] == 'RU', call('Status')
            reset()
        print('VS Code: default Pause, trailing spaces, undo, automatic correction, j → о and following layout OK', flush=True)
    except Exception:
        print('Document:', repr(document.read_text()), 'Status:', call('Status'), flush=True)
        if os.environ.get('PUNTO_TEST_SCREENSHOT'):
            import gi
            gi.require_version('Gtk', '3.0')
            from gi.repository import Gdk, Gtk
            Gtk.init([])
            screen = Gdk.get_default_root_window()
            Gdk.pixbuf_get_from_window(screen, 0, 0, screen.get_width(), screen.get_height()).savev(
                os.environ['PUNTO_TEST_SCREENSHOT'], 'png', [], [])
        print((root / 'code.log').read_text()[-2000:], flush=True)
        raise
    finally:
        editor.terminate()
        daemon.terminate()
        editor.wait(timeout=5)
        daemon.wait(timeout=5)
