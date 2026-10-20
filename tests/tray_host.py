#!/usr/bin/env python3
"""StatusNotifier watcher for the startup regression test."""
from gi.repository import Gio, GLib

XML = """<node><interface name="org.kde.StatusNotifierWatcher">
<method name="RegisterStatusNotifierItem"><arg direction="in" type="s"/></method>
<method name="RegisterStatusNotifierHost"><arg direction="in" type="s"/></method>
<property name="RegisteredStatusNotifierItems" type="as" access="read"/>
<property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
<property name="ProtocolVersion" type="i" access="read"/>
</interface></node>"""
items = []

def call(connection, sender, path, interface, method, parameters, invocation):
    if method == "RegisterStatusNotifierItem":
        item = parameters.unpack()[0]
        items.append(sender + item if item.startswith("/") else item)
    invocation.return_value(None)

def get_property(connection, sender, path, interface, name):
    return {"RegisteredStatusNotifierItems": GLib.Variant("as", items),
            "IsStatusNotifierHostRegistered": GLib.Variant("b", True),
            "ProtocolVersion": GLib.Variant("i", 0)}[name]

connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
connection.register_object("/StatusNotifierWatcher", Gio.DBusNodeInfo.new_for_xml(XML).interfaces[0], call, get_property, None)
Gio.bus_own_name_on_connection(connection, "org.kde.StatusNotifierWatcher", Gio.BusNameOwnerFlags.NONE, None, None)
GLib.MainLoop().run()
