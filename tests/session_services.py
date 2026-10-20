#!/usr/bin/env python3
"""Protocol fixtures for portal/Fcitx5/GNOME/Plasma; never a real compositor."""
import ctypes
import sys
from gi.repository import Gio, GLib

bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
driver = sys.argv[1]
current = 'us'
session = '/org/freedesktop/portal/desktop/session/test/punto'
calls = []
denied = False
XML = '''<node>
<interface name="org.freedesktop.portal.GlobalShortcuts">
<property name="version" type="u" access="read"/>
<method name="CreateSession"><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
<method name="BindShortcuts"><arg type="o" direction="in"/><arg type="a(sa{sv})" direction="in"/><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
<signal name="Activated"><arg type="o"/><arg type="s"/><arg type="t"/><arg type="a{sv}"/></signal>
</interface>
<interface name="org.punto.TestServices">
<method name="EmitAction"><arg type="s" direction="in"/></method>
<method name="GetCalls"><arg type="as" direction="out"/></method>
<method name="Deny"><arg type="b" direction="in"/></method>
</interface>
<interface name="org.punto.Switcher.Focus">
<method name="GetFocus"><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="b" direction="out"/></method>
<method name="GetLayout"><arg type="s" direction="out"/></method>
<method name="SetLayout"><arg type="s" direction="in"/><arg type="b" direction="out"/></method>
</interface>
<interface name="org.kde.KeyboardLayouts">
<method name="getLayout"><arg type="u" direction="out"/></method>
<method name="getLayoutsList"><arg type="a(sss)" direction="out"/></method>
<method name="setLayout"><arg type="u" direction="in"/><arg type="b" direction="out"/></method>
</interface>
<interface name="org.fcitx.Fcitx.Controller1">
<method name="CurrentInputMethod"><arg type="s" direction="out"/></method>
<method name="CurrentInputMethodGroup"><arg type="s" direction="out"/></method>
<method name="InputMethodGroupInfo"><arg type="s" direction="in"/><arg type="s" direction="out"/><arg type="a(ss)" direction="out"/></method>
<method name="SetCurrentIM"><arg type="s" direction="in"/></method>
</interface>
</node>'''
xlib = ctypes.CDLL('libX11.so.6')
xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
xlib.XOpenDisplay.restype = ctypes.c_void_p
display = xlib.XOpenDisplay(None)
xlib.XkbLockGroup.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_uint]
xlib.XFlush.argtypes = [ctypes.c_void_p]
class State(ctypes.Structure):
    _fields_ = [('group', ctypes.c_ubyte), ('locked_group', ctypes.c_ubyte), ('base_group', ctypes.c_ushort), ('latched_group', ctypes.c_ushort), ('mods', ctypes.c_ubyte), ('base_mods', ctypes.c_ubyte), ('latched_mods', ctypes.c_ubyte), ('locked_mods', ctypes.c_ubyte), ('compat_state', ctypes.c_ubyte), ('grab_mods', ctypes.c_ubyte), ('compat_grab_mods', ctypes.c_ubyte), ('lookup_mods', ctypes.c_ubyte), ('compat_lookup_mods', ctypes.c_ubyte), ('ptr_buttons', ctypes.c_ushort)]
xlib.XkbGetState.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.POINTER(State)]
def language():
    state = State()
    assert xlib.XkbGetState(display, 0x100, ctypes.byref(state)) == 0
    return 'ru' if state.group == 1 else 'us'
def switch(layout):
    assert layout in ('us', 'ru')
    assert xlib.XkbLockGroup(display, 0x100, int(layout == 'ru'))
    xlib.XFlush(display)
    calls.append('switch:' + layout)
def response(sender, path, values):
    bus.emit_signal(sender, path, 'org.freedesktop.portal.Request', 'Response', GLib.Variant('(ua{sv})', (1 if denied else 0, values)))
    return False

def call(connection, sender, path, interface, method, params, invocation):
    global denied
    args = params.unpack()
    if method == 'CreateSession':
        calls.append('portal:create')
        request = '/org/freedesktop/portal/desktop/request/test/create'
        invocation.return_value(GLib.Variant('(o)', (request,)))
        GLib.idle_add(response, sender, request, {'session_handle': GLib.Variant('s', session)})
    elif method == 'BindShortcuts':
        assert args[0] == session and len(args[1]) == 8
        calls.append('portal:bind')
        request = '/org/freedesktop/portal/desktop/request/test/bind'
        invocation.return_value(GLib.Variant('(o)', (request,)))
        shortcuts = [(item[0], {'description': GLib.Variant('s', 'test')}) for item in args[1]]
        GLib.idle_add(response, sender, request, {'shortcuts': GLib.Variant('a(sa{sv})', shortcuts)})
    elif method == 'EmitAction':
        bus.emit_signal(None, '/org/freedesktop/portal/desktop', 'org.freedesktop.portal.GlobalShortcuts', 'Activated', GLib.Variant('(osta{sv})', (session, args[0], 0, {})))
        invocation.return_value(None)
    elif method == 'Deny':
        denied = args[0]
        invocation.return_value(None)
    elif method == 'GetCalls':
        invocation.return_value(GLib.Variant('(as)', (calls,)))
    elif method == 'GetFocus':
        invocation.return_value(GLib.Variant('(ssb)', ('NativeTestEditor', 'Test', False)))
    elif method == 'GetLayout':
        invocation.return_value(GLib.Variant('(s)', (language(),)))
    elif method == 'SetLayout':
        switch(args[0]); invocation.return_value(GLib.Variant('(b)', (True,)))
    elif method == 'getLayout':
        invocation.return_value(GLib.Variant('(u)', (int(language() == 'ru'),)))
    elif method == 'getLayoutsList':
        invocation.return_value(GLib.Variant('(a(sss))', ([('us', 'EN', 'English'), ('ru', 'RU', 'Russian')],)))
    elif method == 'setLayout':
        switch(['us', 'ru'][args[0]]); invocation.return_value(GLib.Variant('(b)', (True,)))
    elif method == 'CurrentInputMethod':
        invocation.return_value(GLib.Variant('(s)', ('keyboard-' + language(),)))
    elif method == 'CurrentInputMethodGroup':
        invocation.return_value(GLib.Variant('(s)', ('Existing',)))
    elif method == 'InputMethodGroupInfo':
        assert args[0] == 'Existing'
        invocation.return_value(GLib.Variant('(sa(ss))', ('us', [('keyboard-us', ''), ('keyboard-ru', '')])))
    elif method == 'SetCurrentIM':
        switch(args[0].removeprefix('keyboard-')); invocation.return_value(None)
    else:
        raise AssertionError(method)

def register(path, interface):
    info = Gio.DBusNodeInfo.new_for_xml(XML)
    selected = next(item for item in info.interfaces if item.name == interface)
    bus.register_object(path, selected, call, lambda *_: GLib.Variant('u', 1), None)
register('/org/freedesktop/portal/desktop', 'org.freedesktop.portal.GlobalShortcuts')
register('/test', 'org.punto.TestServices')
for name in ['org.freedesktop.portal.Desktop', 'org.punto.TestServices']:
    Gio.bus_own_name_on_connection(bus, name, Gio.BusNameOwnerFlags.NONE, None, None)
name, path, interface = {
    'gnome': ('org.punto.Switcher.Focus', '/org/punto/Switcher/Focus', 'org.punto.Switcher.Focus'),
    'kde': ('org.kde.keyboard', '/Layouts', 'org.kde.KeyboardLayouts'),
    'fcitx': ('org.fcitx.Fcitx5', '/controller', 'org.fcitx.Fcitx.Controller1'),
}[driver]
register(path, interface)
Gio.bus_own_name_on_connection(bus, name, Gio.BusNameOwnerFlags.NONE, None, None)
GLib.MainLoop().run()
