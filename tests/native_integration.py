#!/usr/bin/env python3
"""Run only on a disposable X server/session bus (Xvfb + dbus-run-session)."""
import ctypes
import os
import pathlib
import subprocess
import sys
import tempfile
import time

sandbox = tempfile.TemporaryDirectory(prefix='punto-native-')
os.environ['HOME'] = sandbox.name
os.environ['XDG_CONFIG_HOME'] = sandbox.name
os.environ['GTK_IM_MODULE'] = 'xim'
os.environ['XMODIFIERS'] = '@im=none'
os.environ['NO_AT_BRIDGE'] = '0'
os.environ['GTK_MODULES'] = 'atk-bridge'

import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, Gio, GLib


def pump(until=lambda: False, timeout=6):
    deadline = time.monotonic() + timeout
    context = GLib.MainContext.default()
    stable_since = None
    while time.monotonic() < deadline:
        while context.pending():
            context.iteration(False)
        if until():
            if stable_since is None:
                stable_since = time.monotonic()
            if time.monotonic() - stable_since >= 0.1:
                return
        else:
            stable_since = None
        time.sleep(0.01)
    assert until(), 'Timed out waiting for native input'


with sandbox as temp:
    os.environ['HOME'] = temp
    input_config = pathlib.Path(temp) / '.xinputrc'
    input_config.write_text('run_im xim\n')
    os.environ['XDG_CONFIG_HOME'] = temp
    driver = os.environ.get('PUNTO_TEST_DRIVER')
    os.environ['XDG_SESSION_TYPE'] = 'wayland' if driver else 'x11'
    os.environ['GTK_IM_MODULE'] = 'xim'
    os.environ['XMODIFIERS'] = '@im=none'
    os.environ['NO_AT_BRIDGE'] = '0'
    os.environ['GTK_MODULES'] = 'atk-bridge'
    config = pathlib.Path(temp) / 'punto-rs'
    config.mkdir()
    (config / 'config.toml').write_text('backend = "native"\nremember_layout = false\n')
    session = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    def call(method, signature=None, args=()):
        return session.call_sync('org.punto.Switcher', '/org/punto/Switcher', 'org.punto.Switcher', method, GLib.Variant(signature, args) if signature else None, None, Gio.DBusCallFlags.NONE, 2000, None).unpack()
    def owner(name):
        return session.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus', 'NameHasOwner', GLib.Variant('(s)', (name,)), None, Gio.DBusCallFlags.NONE, 1000, None).unpack()[0]
    def service(method, params=None):
        return session.call_sync('org.punto.TestServices', '/test', 'org.punto.TestServices', method, params, None, Gio.DBusCallFlags.NONE, 1000, None).unpack()
    host = subprocess.Popen([sys.executable, str(pathlib.Path(__file__).with_name('tray_host.py'))])
    pump(lambda: owner('org.kde.StatusNotifierWatcher'))
    window = Gtk.Window(title='Punto-Native-Integration')
    window.set_default_size(600, 300)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    view = Gtk.TextView()
    view.set_vexpand(True)
    password = Gtk.Entry()
    password.set_visibility(False)
    box.add(view)
    box.add(password)
    window.add(box)
    window.show_all()
    view.grab_focus()
    pump(lambda: window.get_realized())
    subprocess.run(['setxkbmap', '-layout', 'us,ru', '-option', '', '-option', 'grp:ctrl_shift_toggle'], check=True)
    mapping_before = subprocess.check_output(['setxkbmap', '-query'])
    services = subprocess.Popen([sys.executable, str(pathlib.Path(__file__).with_name('session_services.py')), driver]) if driver else None
    if services:
        pump(lambda: owner('org.punto.TestServices'))
    xid = subprocess.check_output(['xdotool', 'search', '--name', 'Punto-Native-Integration']).decode().strip().splitlines()[-1]
    subprocess.run(['xdotool', 'windowfocus', xid], check=True)
    engine = subprocess.Popen([sys.argv[1], '--daemon'])
    try:
        pump(lambda: owner('org.punto.Switcher'))
        pump(lambda: call('InputReady')[0])
        assert call('InputBackend')[0] == ('AT-SPI (Wayland)' if driver else 'X11 + AT-SPI')
        if services:
            call('ConnectShortcuts')
            pump(lambda: 'подключены через портал' in call('Status')[3])
        assert not owner('org.freedesktop.IBus'), 'Native backend started IBus'
        # Deliver physical keycodes: XKB chooses the existing system group.
        xlib = ctypes.CDLL('libX11.so.6')
        xtest = ctypes.CDLL('libXtst.so.6')
        xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
        xlib.XOpenDisplay.restype = ctypes.c_void_p
        display = xlib.XOpenDisplay(None)
        xlib.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        xlib.XKeysymToKeycode.restype = ctypes.c_ubyte
        xlib.XFlush.argtypes = [ctypes.c_void_p]
        xtest.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        def physical(text):
            for char in text:
                code = xlib.XKeysymToKeycode(display, ord(char))
                assert code
                xtest.XTestFakeKeyEvent(display, code, True, 0)
                xtest.XTestFakeKeyEvent(display, code, False, 0)
                xlib.XFlush(display)
                deadline = time.monotonic() + 0.025
                while time.monotonic() < deadline:
                    while GLib.MainContext.default().pending():
                        GLib.MainContext.default().iteration(False)
                    time.sleep(0.002)
        def key(chord):
            if services:
                assert chord == 'Scroll_Lock'
                service('EmitAction', GLib.Variant('(s)', ('undo',)))
            else:
                subprocess.run(['xdotool', 'key', '--clearmodifiers', chord], check=True)
        def convert_selection():
            if services:
                service('EmitAction', GLib.Variant('(s)', ('convert_selection',)))
                return
            symbols = [0xffe3, 0xffe9, ord('s')]
            for symbol in symbols:
                xtest.XTestFakeKeyEvent(display, xlib.XKeysymToKeycode(display, symbol), True, 0)
            for symbol in reversed(symbols):
                xtest.XTestFakeKeyEvent(display, xlib.XKeysymToKeycode(display, symbol), False, 0)
            xlib.XFlush(display)
        buffer = view.get_buffer()
        clipboard = Gtk.Clipboard.get_default(window.get_display())
        clipboard.set_text('preserve native clipboard', -1)
        text = lambda: buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True)
        physical('lf ')
        pump(lambda: text() == 'да ' and call('Status')[0] == 'RU')
        physical('lf ')
        pump(lambda: text() == 'да да ')
        buffer.set_text('')
        physical('we ')
        pump(lambda: text() == 'we ' and call('Status')[0] == 'EN')
        physical('we ')
        pump(lambda: text() == 'we we ')
        for shifted in (True, False):
            call('ActivateAction', '(s)', ('switch_layout',))
            pump(lambda: call('Status')[0] == 'RU')
            buffer.set_text('')
            if shifted:
                xtest.XTestFakeKeyEvent(display, xlib.XKeysymToKeycode(display, 0xffe1), True, 0)
            physical('i')
            if shifted:
                xtest.XTestFakeKeyEvent(display, xlib.XKeysymToKeycode(display, 0xffe1), False, 0)
            physical(' ')
            pump(lambda: text() == 'I ' and call('Status')[0] == 'EN')
            physical('we ')
            pump(lambda: text() == 'I we ')
        for original, expected in [('ghjdthz. ', 'проверяю '), ('k.,k. ', 'люблю '), (';bpym ', 'жизнь ')]:
            buffer.set_text('')
            physical(original)
            pump(lambda: text() == expected and call('Status')[0] == 'RU')
            key('Scroll_Lock')
            pump(lambda: text() == original and call('Status')[0] == 'EN')
        buffer.set_text('')
        physical('ghbdtn ')
        try:
            pump(lambda: text() == 'привет ' and call('Status')[0] == 'RU')
        except AssertionError:
            print('Actual:', repr(text()), 'Status:', call('Status'), flush=True)
            raise
        assert call('Status')[0] == 'RU'
        key('Scroll_Lock')
        pump(lambda: text() == 'ghbdtn ' and call('Status')[0] == 'EN')
        assert call('Status')[0] == 'EN'
        buffer.set_text('')
        for symbol, pressed in [(0xffe3, True), (0xffe1, True), (0xffe1, False), (0xffe3, False)]:
            xtest.XTestFakeKeyEvent(display, xlib.XKeysymToKeycode(display, symbol), pressed, 0)
        xlib.XFlush(display)
        pump(lambda: call('Status')[0] == 'RU')
        physical('hello ')
        pump(lambda: text() == 'hello ')
        key('Scroll_Lock')
        pump(lambda: text() == 'руддщ ')
        # Convert selection and verify undo without using CLIPBOARD.
        buffer.set_text('ghbdtn')
        buffer.select_range(buffer.get_start_iter(), buffer.get_end_iter())
        convert_selection()
        pump(lambda: text() == 'привет')
        key('Scroll_Lock')
        pump(lambda: text() == 'ghbdtn')
        # A moved cursor must not allow deletion by undo.
        buffer.select_range(buffer.get_start_iter(), buffer.get_end_iter())
        convert_selection()
        pump(lambda: text() == 'привет')
        buffer.place_cursor(buffer.get_start_iter())
        key('Scroll_Lock')
        pump(lambda: any(word in call('Status')[3].lower() for word in ('откат', 'отмен')))
        assert text() == 'привет'
        # The password field is skipped even when its contents resemble a wrong-layout word.
        password.grab_focus()
        password.set_text('')
        call('ActivateAction', '(s)', ('switch_layout',))
        pump(lambda: call('Status')[0] == 'EN')
        physical('ghbdtn ')
        pump(lambda: password.get_text() == 'ghbdtn ')
        assert not call('Status')[1], 'Password field reports automatic processing'
        view.grab_focus()
        buffer.set_text('')
        (config / 'config.toml').write_text('backend = "native"\nremember_layout = false\n[[exceptions]]\napp = "native_integration.py"\ndisable_auto = true\n')
        call('Reload')
        physical('ghbdtn ')
        pump(lambda: text() == 'ghbdtn ')
        assert not call('Status')[1], 'Reloaded application exception was not applied'
        assert subprocess.check_output(['setxkbmap', '-query']) == mapping_before, 'Native backend changed XKB layouts/options'
        assert not owner('org.freedesktop.IBus')
        assert input_config.read_text() == 'run_im xim\n'
        assert clipboard.wait_for_text() == 'preserve native clipboard'
        if services:
            calls = service('GetCalls')[0]
            assert 'portal:create' in calls and 'portal:bind' in calls and 'switch:ru' in calls and 'switch:us' in calls
            service('Deny', GLib.Variant('(b)', (True,)))
            call('ConnectShortcuts')
            pump(lambda: 'отклонено' in call('Status')[3])
        print(f'Native {driver or "X11"}: both auto-correction directions, system Ctrl+Shift, selection, verified undo, password bypass and unchanged XKB configuration OK', flush=True)
    except BaseException:
        if owner('org.punto.Switcher'):
            print('Failure details:', repr(text()), buffer.get_selection_bounds(), call('Status'), flush=True)
        raise
    finally:
        if engine.poll() is None:
            engine.terminate()
            engine.wait(timeout=5)
        window.destroy()
        host.terminate()
        host.wait(timeout=5)
        if services:
            services.terminate()
            services.wait(timeout=5)
