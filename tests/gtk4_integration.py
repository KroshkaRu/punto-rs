#!/usr/bin/env python3
"""GNOME Text Editor 46.x, physical keys and AT-SPI, on a disposable X11/D-Bus session."""
import ctypes
import os
import pathlib
import subprocess
import sys
import tempfile
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
    assert until(), 'GTK4 integration timed out'

def editable(node):
    try:
        if node.get_text_iface() and node.get_editable_text_iface() and node.get_state_set().contains(Atspi.StateType.EDITABLE) and node.get_state_set().contains(Atspi.StateType.FOCUSED):
            return node
        for index in range(node.get_child_count()):
            found = editable(node.get_child_at_index(index))
            if found:
                return found
    except GLib.Error:
        pass
    return None

with tempfile.TemporaryDirectory(prefix='punto-gtk4-') as directory:
    os.environ.update(HOME=directory, XDG_CONFIG_HOME=directory, GTK_IM_MODULE='xim', XMODIFIERS='@im=none', NO_AT_BRIDGE='0')
    config = pathlib.Path(directory) / 'punto-rs'
    config.mkdir()
    (config / 'config.toml').write_text('backend = "native"\nremember_layout = true\n')
    document = pathlib.Path(directory) / 'test.txt'
    document.write_text('')
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
        return bus.call_sync('org.punto.Switcher', '/org/punto/Switcher', 'org.punto.Switcher', method, args, None, Gio.DBusCallFlags.NONE, 1000, None).unpack()
    host = subprocess.Popen([sys.executable, str(pathlib.Path(__file__).with_name('tray_host.py'))])
    editor = subprocess.Popen(['gnome-text-editor', '--standalone', str(document)])
    daemon = subprocess.Popen([sys.argv[1], '--daemon'])
    try:
        field = None
        def find():
            global field
            field = editable(Atspi.get_desktop(0))
            return field is not None
        pump(find)
        text = lambda: Atspi.Text.get_text(field.get_text_iface(), 0, -1)
        xid = subprocess.check_output(['xdotool', 'search', '--name', 'test.txt']).decode().splitlines()[-1]
        subprocess.run(['xdotool', 'windowactivate', '--sync', xid], check=True)
        def ready():
            try: return call('InputReady')[0]
            except GLib.Error: return False
        pump(ready)
        def physical(value):
            for char in value:
                code = xlib.XKeysymToKeycode(display, ord(char))
                for pressed in (True, False):
                    xtest.XTestFakeKeyEvent(display, code, pressed, 0)
                xlib.XFlush(display)
                time.sleep(0.04)
        physical('ghbdtn ')
        pump(lambda: text() == 'привет ')
        physical('lf ')
        pump(lambda: text() == 'привет да ')
        physical('we ')
        pump(lambda: text() == 'привет да we ')
        physical('we ')
        pump(lambda: text() == 'привет да we we ')
        assert subprocess.check_output(['setxkbmap', '-query']) == mapping
        print('GNOME Text Editor: both corrections and following physical input in the new language OK', flush=True)
    except BaseException:
        print('Actual text:', repr(text()) if field else None, 'Daemon exit:', daemon.poll(), flush=True)
        if daemon.poll() is None: print('Status:', call('Status'), flush=True)
        raise
    finally:
        for process in (editor, daemon, host):
            process.terminate()
            process.wait(timeout=5)
