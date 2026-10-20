#!/usr/bin/env python3
"""Chromium/Electron Text-only fields, physical keys and AT-SPI, on a disposable X11/D-Bus session."""
import contextlib
from sound_fixture import audio_server
import json
import ctypes
import os
import pathlib
import subprocess
import sys
import tempfile
import shutil
import time
import gi
gi.require_version('Atspi', '2.0')
from gi.repository import Atspi, Gio, GLib

def pump(until, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        while GLib.MainContext.default().pending():
            GLib.MainContext.default().iteration(False)
        if until():
            return
        time.sleep(0.02)
    assert until(), 'Electron integration timed out'

def editable(node):
    try:
        if node.get_text_iface() and node.get_state_set().contains(Atspi.StateType.EDITABLE) and node.get_state_set().contains(Atspi.StateType.FOCUSED):
            return node
        for index in range(node.get_child_count()):
            found = editable(node.get_child_at_index(index))
            if found:
                return found
    except GLib.Error:
        pass
    return None

with contextlib.ExitStack() as cleanup, tempfile.TemporaryDirectory(prefix='punto-electron-') as directory:
    os.environ.update(ACCESSIBILITY_ENABLED='1', HOME=directory, XDG_CONFIG_HOME=directory, GTK_IM_MODULE='xim', XMODIFIERS='@im=none', NO_AT_BRIDGE='0')
    os.environ['XDG_SESSION_TYPE'] = os.environ.get('PUNTO_TEST_SESSION', 'x11')
    raw = os.environ.get('PUNTO_TEST_RAW') == '1'
    if raw:
        os.environ['ACCESSIBILITY_ENABLED'] = '0'
        os.environ['PUNTO_TEST_OUTPUT'] = directory + '/output.json'
        recorded_sound = cleanup.enter_context(audio_server())
    config = pathlib.Path(directory) / 'punto-rs'
    config.mkdir()
    (config / 'config.toml').write_text('backend = "native"\nremember_layout = true\n' + ('[[hotkeys]]\naction = "convert_word"\nkeys = "Ctrl+Super+W"\n[[hotkeys]]\naction = "undo"\nkeys = "Ctrl+Super+Z"\n[[hotkeys]]\naction = "convert_selection"\nkeys = "Ctrl+Super+S"\n' if raw else ''))
    runtime = pathlib.Path(os.environ['PUNTO_TEST_ELECTRON']).resolve()
    fixture = pathlib.Path(directory) / 'electron'
    fixture.mkdir()
    shutil.copy2(runtime, fixture / 'electron')
    for item in runtime.parent.iterdir():
        if item.name not in ('resources', runtime.name):
            (fixture / item.name).symlink_to(item)
    shutil.copytree(pathlib.Path(__file__).with_name('electron-fixture'), fixture / 'resources/app')
    xlib = ctypes.CDLL('libX11.so.6')
    xtest = ctypes.CDLL('libXtst.so.6')
    xlib.XOpenDisplay.restype = ctypes.c_void_p
    display = xlib.XOpenDisplay(None)
    xlib.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    xlib.XKeysymToKeycode.restype = ctypes.c_ubyte
    xtest.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
    xlib.XFlush.argtypes = [ctypes.c_void_p]
    subprocess.run(['setxkbmap', '-layout', 'us,ru', '-option', '', '-option', 'grp:ctrl_shift_toggle'], check=True)
    mapping = subprocess.check_output(['setxkbmap', '-query'])
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    def call(method, args=None):
        return bus.call_sync('org.punto.Switcher', '/org/punto/Switcher', 'org.punto.Switcher', method, args, None, Gio.DBusCallFlags.NONE, 10000 if method == 'Reload' else 1000, None).unpack()
    host = subprocess.Popen([sys.executable, str(pathlib.Path(__file__).with_name('tray_host.py'))])
    editor = subprocess.Popen([str(fixture / 'electron'), '--no-sandbox', '--disable-gpu', '--ozone-platform=x11', *(['--force-renderer-accessibility'] if not raw else []), '--user-data-dir=' + directory + '/profile'], stderr=open(directory + '/electron.log', 'w'))
    daemon = subprocess.Popen([sys.argv[1], '--daemon'])
    try:
        field = None
        def find():
            global field
            field = editable(Atspi.get_desktop(0))
            return field is not None
        if raw:
            output = pathlib.Path(os.environ['PUNTO_TEST_OUTPUT'])
            pump(output.exists)
            def contents():
                try: return json.loads(output.read_text())
                except (OSError, json.JSONDecodeError): return {}
            text = lambda: contents().get('text')
            assert not find(), 'Raw fixture unexpectedly exposed an accessible focused field'
        else:
            pump(find)
            assert 'EditableText' not in field.get_interfaces(), 'Fixture does not exercise the Text-only path'
            text = lambda: Atspi.Text.get_text(field.get_text_iface(), 0, -1)
        xid = subprocess.check_output(['xdotool', 'search', '--name', 'Punto Electron Test']).decode().splitlines()[-1]
        subprocess.run(['xdotool', 'windowfocus', '--sync', xid], check=True)
        def ready():
            try: return call('InputReady')[0]
            except GLib.Error: return False
        pump(ready)
        def physical(value, delay=0.04, hold_space=0.08):
            for char in value:
                code = xlib.XKeysymToKeycode(display, ord(char))
                for pressed in (True, False):
                    xtest.XTestFakeKeyEvent(display, code, pressed, 0)
                    if char == ' ' and pressed:
                        xlib.XFlush(display)
                        time.sleep(hold_space)  # A real user keeps Space held while AT-SPI publishes insertion.
                xlib.XFlush(display)
                time.sleep(delay)
        if os.environ['XDG_SESSION_TYPE'] == 'wayland':
            physical('ghbdtn ')
            pump(lambda: text() == '😀 ghbdtn ')
            assert not call('Status')[1], 'Text-only field advertised unsupported Wayland editing'
            print('Wayland backend refuses Text-only editing without X11 fallback OK', flush=True)
        else:
            physical('ghbdtn ')
            pump(lambda: text() == '😀 привет ')
            physical('lf ')
            pump(lambda: text() == '😀 привет да ')
            physical('we ')
            pump(lambda: text() == '😀 привет да we ')
            physical('we ')
            pump(lambda: text() == '😀 привет да we we ')
            assert subprocess.check_output(['setxkbmap', '-query']) == mapping
            physical('lf ')
            pump(lambda: text() == '😀 привет да we we да ')
            call('ActivateAction', GLib.Variant('(s)', ('undo',)))
            pump(lambda: text() == '😀 привет да we we lf ')
            subprocess.run(['xdotool', 'key', 'ctrl+shift'], check=True)
            physical('lf ')
            pump(lambda: text().endswith('да '))
            assert text().startswith('😀 ')
            subprocess.run(['xdotool', 'key', 'Tab'], check=True)
            time.sleep(0.2)
            if raw: call('SetLayout', GLib.Variant('(s)', ('EN',)))
            physical('ghbdtn ')
            if raw:
                # Without AT-SPI, even password fields follow the explicitly documented unknown-field policy.
                pump(lambda: contents().get('password') == 'привет ')
                call('ActivateAction', GLib.Variant('(s)', ('undo',)))
                pump(lambda: contents().get('password') == 'ghbdtn ')
                recorded_sound(4)
                assert not find(), 'Accessibility was enabled by the backend'
                print('Generic X11 without AT-SPI: corrections, layout, undo, sound and unchanged accessibility OK', flush=True)
            else:
                pump(lambda: not call('Status')[1])
                print('Electron Text-only: corrections, following layout, undo and system Ctrl+Shift OK', flush=True)
            assert text().endswith('да '), 'Password input changed the previous text field'
            if raw:
                subprocess.run(['xdotool', 'key', 'shift+Tab', 'ctrl+a', 'BackSpace'], check=True)
                pump(lambda: text() == '')
                call('SetLayout', GLib.Variant('(s)', ('EN',)))
                physical('ghbdtn', delay=0.005)
                pump(lambda: text() == 'ghbdtn')
                subprocess.run(['xdotool', 'key', 'ctrl+a'], check=True)
                call('ActivateAction', GLib.Variant('(s)', ('convert_selection',)))
                pump(lambda: text() == 'привет')
                call('ActivateAction', GLib.Variant('(s)', ('undo',)))
                pump(lambda: text() == 'ghbdtn')
                subprocess.run(['xdotool', 'key', 'ctrl+a', 'BackSpace'], check=True)
                physical('ghbdtn', delay=0.005)
                call('ActivateAction', GLib.Variant('(s)', ('convert_word',)))
                pump(lambda: text() == 'привет')
                call('ActivateAction', GLib.Variant('(s)', ('undo',)))
                pump(lambda: text() == 'ghbdtn')
                subprocess.run(['xdotool', 'key', 'Left'], check=True)
                call('ActivateAction', GLib.Variant('(s)', ('undo',)))
                time.sleep(0.1)
                assert text() == 'ghbdtn', 'Undo after cursor navigation changed text'
                for _ in range(8):
                    subprocess.run(['xdotool', 'key', 'ctrl+a', 'BackSpace'], check=True)
                    call('SetLayout', GLib.Variant('(s)', ('EN',)))
                    physical('ghbdtn ghbdtn ghbdtn ', delay=0.003, hold_space=0)
                    pump(lambda: text().endswith(' '))
                    time.sleep(0.15)
                    assert len(text().split()) == 3 and all(word in ('ghbdtn', 'привет') for word in text().split()), repr(text())
                def chord(letter):
                    symbols = [0xffe3, 0xffeb, ord(letter)]
                    for symbol in symbols:
                        xtest.XTestFakeKeyEvent(display, xlib.XKeysymToKeycode(display, symbol), True, 0)
                    for symbol in reversed(symbols):
                        xtest.XTestFakeKeyEvent(display, xlib.XKeysymToKeycode(display, symbol), False, 0)
                    xlib.XFlush(display)
                subprocess.run(['xdotool', 'key', 'ctrl+a', 'BackSpace'], check=True)
                call('SetLayout', GLib.Variant('(s)', ('EN',)))
                physical('hello ')
                pump(lambda: text() == 'hello ')
                chord('w')
                pump(lambda: text() == 'руддщ ')
                chord('z')
                pump(lambda: text() == 'hello ')
                print('Generic X11: real custom hotkeys in both layouts and last completed word OK', flush=True)
                call('SetLayout', GLib.Variant('(s)', ('RU',)))
                physical('ghbdtn ')
                pump(lambda: text() == 'hello привет ')
                subprocess.run(['xdotool', 'key', 'ctrl+a'], check=True)
                chord('s')
                pump(lambda: text() == 'руддщ ghbdtn ')
                chord('z')
                pump(lambda: text() == 'hello привет ')
                print('Generic X11: mixed RU/EN selection and undo preserve each script OK', flush=True)
                for source, language, expected in [('ghjdthz. ', 'EN', 'проверяю '), ('ghbdtn. ', 'EN', 'привет. ')]:
                    subprocess.run(['xdotool', 'key', 'ctrl+a', 'BackSpace'], check=True)
                    call('SetLayout', GLib.Variant('(s)', (language,)))
                    physical(source)
                    pump(lambda: text() == expected)
                subprocess.run(['xdotool', 'key', 'ctrl+a', 'BackSpace'], check=True)
                call('SetLayout', GLib.Variant('(s)', ('RU',)))
                shift = xlib.XKeysymToKeycode(display, 0xffe1)
                xtest.XTestFakeKeyEvent(display, shift, True, 0)
                xlib.XFlush(display)
                physical('i')
                xtest.XTestFakeKeyEvent(display, shift, False, 0)
                xlib.XFlush(display)
                physical(' ')
                pump(lambda: text() == 'I ')
                (config / 'config.toml').write_text('backend = "native"\nx11_unverified = true\n[[exceptions]]\napp = "punto-electron-test"\ndisable_auto = true\n')
                call('Reload')
                subprocess.run(['xdotool', 'key', 'ctrl+a', 'BackSpace'], check=True)
                call('SetLayout', GLib.Variant('(s)', ('EN',)))
                physical('ghbdtn ')
                pump(lambda: text() == 'ghbdtn ')
                assert not call('Status')[1], 'Application exception did not disable generic correction'
                (config / 'config.toml').write_text('backend = "native"\nx11_unverified = false\n')
                call('Reload')
                subprocess.run(['xdotool', 'key', 'ctrl+a', 'BackSpace'], check=True)
                call('SetLayout', GLib.Variant('(s)', ('EN',)))
                physical('ghbdtn ')
                pump(lambda: text() == 'ghbdtn ')
                assert not call('Status')[1], 'Disabled generic backend still advertised automatic input'
                print('Generic X11: PRIMARY selection, manual word, safe undo cancellation and fast typing preserve text OK', flush=True)
    except BaseException:
        print('Actual text:', repr(text()) if field or raw else None, 'Daemon exit:', daemon.poll(), flush=True)
        if daemon.poll() is None: print('Status:', call('Status'), 'Output:', contents() if raw else None, flush=True)
        raise
    finally:
        for process in (editor, daemon, host):
            process.terminate()
            process.wait(timeout=5)
