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
        subprocess.run(['xdotool', 'key', 'ctrl+a'], check=True)
        call('ActivateAction', GLib.Variant('(s)', ('transliterate',)))
        pump(lambda: text() == 'privet da ве ве ')
        call('ActivateAction', GLib.Variant('(s)', ('undo',)))
        pump(lambda: text() == 'привет да we we ')
        def held_chord(modifier, key, before, duration):
            subprocess.run(['xdotool', 'keydown', modifier, 'keydown', key], check=True)
            time.sleep(duration)
            assert text() == before, 'Hotkey edited while the keyboard grab was active'
            subprocess.run(['xdotool', 'keyup', key], check=True)
            time.sleep(0.15)
            assert text() == before, 'Hotkey edited while its modifier was still held'
            subprocess.run(['xdotool', 'keyup', modifier], check=True)
        for duration in (0.3, 0.8):
            Atspi.EditableText.set_text_contents(field.get_editable_text_iface(), 'Privet, shchuka!')
            subprocess.run(['xdotool', 'key', 'ctrl+a'], check=True)
            held_chord('Alt_L', 'Next', 'Privet, shchuka!', duration)
            pump(lambda: text() == 'Привет, щука!')
            held_chord('Control_L', 'BackSpace', 'Привет, щука!', duration)
            pump(lambda: text() == 'Privet, shchuka!')
            time.sleep(0.15)
            assert text() == 'Privet, shchuka!', 'Autorepeat executed undo more than once'
        print('Held Alt+PageDown/Ctrl+Backspace, modifier release and autorepeat OK', flush=True)
        Atspi.EditableText.set_text_contents(field.get_editable_text_iface(), 'privet')
        subprocess.run(['xdotool', 'key', 'ctrl+a', 'keydown', 'Alt_L', 'keydown', 'Next'], check=True)
        time.sleep(0.15)
        bounds = Atspi.Component.get_extents(field.get_component_iface(), Atspi.CoordType.SCREEN)
        subprocess.run(['xdotool', 'mousemove', str(bounds.x + 15), str(bounds.y + 15), 'click', '--repeat', '3', '--delay', '50', '1'], check=True)
        subprocess.run(['xdotool', 'keyup', 'Next', 'keyup', 'Alt_L'], check=True)
        time.sleep(0.3)
        assert text() == 'privet', 'Pending hotkey edited after a mouse selection change'
        assert 'отменено' in call('Status')[-1], call('Status')
        print('Mouse selection change cancels a pending hotkey OK', flush=True)
        other = subprocess.Popen([sys.executable, '-c', "import gi; gi.require_version('Gtk', '3.0'); from gi.repository import Gtk; w=Gtk.Window(title='Punto Focus Guard'); e=Gtk.Entry(text='unchanged'); e.select_region(0,-1); w.add(e); w.show_all(); Gtk.main()"])
        try:
            def other_window():
                result = subprocess.run(['xdotool', 'search', '--name', '^Punto Focus Guard$'], capture_output=True, text=True)
                return result.stdout.strip()
            pump(other_window)
            other_id = other_window().splitlines()[-1]
            subprocess.run(['xdotool', 'windowactivate', '--sync', xid], check=True)
            Atspi.EditableText.set_text_contents(field.get_editable_text_iface(), 'privet')
            subprocess.run(['xdotool', 'key', 'ctrl+a', 'keydown', 'Alt_L', 'keydown', 'Next'], check=True)
            time.sleep(0.15)
            subprocess.run(['xdotool', 'windowfocus', other_id], check=True)
            time.sleep(0.8)  # Autorepeat must not bind the old chord to the new window.
            subprocess.run(['xdotool', 'keyup', 'Next', 'keyup', 'Alt_L'], check=True)
            time.sleep(0.3)
            assert text() == 'privet', 'Pending hotkey edited after focus moved to another window'
            def untouched(node):
                try:
                    iface = node.get_text_iface()
                    if iface and Atspi.Text.get_text(iface, 0, -1) == 'unchanged': return True
                    return any(untouched(node.get_child_at_index(i)) for i in range(node.get_child_count()))
                except GLib.Error: return False
            assert untouched(Atspi.get_desktop(0)), 'Pending hotkey changed text in the new window'
            subprocess.run(['xdotool', 'windowactivate', '--sync', xid], check=True)
            subprocess.run(['xdotool', 'key', 'ctrl+a', 'alt+Next'], check=True)
            pump(lambda: text() == 'привет')
            print('Window switch cancels a pending hotkey; the next chord still works OK', flush=True)
        finally:
            other.terminate()
            other.wait(timeout=5)
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
