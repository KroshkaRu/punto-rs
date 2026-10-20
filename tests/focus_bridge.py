#!/usr/bin/env python3
"""Fake compositor identity for headless IBus tests; no input synthesis."""
import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

XML = """<node><interface name="org.punto.Switcher.Focus">
<method name="GetFocus"><arg direction="out" type="s"/><arg direction="out" type="s"/><arg direction="out" type="b"/></method>
<method name="SetFocus"><arg direction="in" type="s"/><arg direction="in" type="s"/><arg direction="in" type="b"/></method>
</interface></node>"""
focus = ("org.punto.TestEditor", "Test editor", False)


def call(connection, sender, path, interface, method, parameters, invocation):
    global focus
    if method == "GetFocus":
        invocation.return_value(GLib.Variant("(ssb)", focus))
    else:
        focus = parameters.unpack()
        invocation.return_value(None)


connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
info = Gio.DBusNodeInfo.new_for_xml(XML).interfaces[0]
connection.register_object("/org/punto/Switcher/Focus", info, call, None, None)
Gio.bus_own_name_on_connection(connection, "org.punto.Switcher.Focus", Gio.BusNameOwnerFlags.NONE, None, None)
GLib.MainLoop().run()
