#!/usr/bin/env python3
"""Exercise the Rust engine through a real, isolated IBus daemon and GI client."""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time

import gi
gi.require_version("IBus", "1.0")
from gi.repository import GLib, IBus


def pump(until=lambda: False, timeout=5):
    deadline = time.monotonic() + timeout
    context = GLib.MainContext.default()
    while time.monotonic() < deadline:
        while context.pending():
            context.iteration(False)
        if until():
            return
        time.sleep(0.005)
    if not until():
        raise AssertionError("Timed out waiting for IBus")


def gtk_check():
    os.environ["GTK_IM_MODULE"] = "ibus"
    gi.require_version("Gtk", "3.0")
    from gi.repository import Gtk
    window = Gtk.Window(title="Punto-GTK-Integration")
    view = Gtk.TextView()
    window.add(view)
    window.show_all()
    view.grab_focus()
    pump(lambda: window.get_realized())
    window_id = subprocess.check_output(["xdotool", "search", "--name", "Punto-GTK-Integration"]).decode().strip().splitlines()[-1]
    subprocess.run(["xdotool", "windowfocus", window_id], check=True)
    buffer = view.get_buffer()
    text = lambda: buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), True)
    def key(keys):
        subprocess.run(["xdotool", "key", "--clearmodifiers", keys], check=True)
    for physical, expected in [("lf lf ", "да да "), ("we we ", "we we ")]:
        buffer.set_text("")
        subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "25", physical], check=True)
        pump(lambda: text() == expected)
    for physical in ("I we ", "i we "):
        buffer.set_text("")
        key("ctrl+alt+space")
        subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "25", physical], check=True)
        pump(lambda: text() == "I we ")
    buffer.set_text("")
    subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "15", "ghbdtn "], check=True)
    pump(lambda: text() == "привет ")
    key("Scroll_Lock")
    pump(lambda: text() == "ghbdtn ")
    buffer.set_text("ghbdtn")
    buffer.select_range(buffer.get_start_iter(), buffer.get_end_iter())
    key("ctrl+alt+s")
    pump(lambda: text() == "привет")
    key("Scroll_Lock")
    pump(lambda: text() == "ghbdtn")
    buffer.set_text("")
    key("ctrl+alt+space")  # RU: physical hello keys produce руддщ before Space.
    subprocess.run(["xdotool", "type", "--clearmodifiers", "--delay", "15", "hello "], check=True)
    pump(lambda: text() == "hello ")
    key("Scroll_Lock")
    pump(lambda: text() == "руддщ ")
    window.destroy()
    print("GTK3 integration: ghbdtn → привет, руддщ → hello, selection replacement and undo OK")


def main():
    binary = pathlib.Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="punto-ibus-test-") as temp:
        root = pathlib.Path(temp)
        component_dir = root / "components"
        component_dir.mkdir()
        for source in pathlib.Path("/usr/share/ibus/component").glob("*.xml"):
            if source.name == "simple.xml":
                shutil.copy(source, component_dir)
        template = pathlib.Path(__file__).resolve().parents[1] / "packaging/punto-rs.xml.in"
        (component_dir / "punto-rs.xml").write_text(template.read_text().replace("@BINARY@", str(binary)))
        env = os.environ.copy()
        env.update(IBUS_COMPONENT_PATH=str(component_dir), XDG_CONFIG_HOME=str(root / "config"),
                   XDG_CACHE_HOME=str(root / "cache"), XDG_DATA_HOME=str(root / "data"), XDG_SESSION_TYPE="wayland")
        # Do not discover or replace the user's IBus, even with DISPLAY inherited.
        env.pop("IBUS_ADDRESS", None)
        env["IBUS_ADDRESS"] = "unix:path=" + str(root / "bus")
        engine = subprocess.Popen([str(binary), "--ibus"], env=env)
        from gi.repository import Gio
        session = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        pump(lambda: session.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", GLib.Variant("(s)", ("org.punto.Switcher",)), None, Gio.DBusCallFlags.NONE, 1000, None).unpack()[0])
        assert engine.poll() is None, "Tray process exited without IBus"
        daemon = subprocess.Popen(["ibus-daemon", "--single", "--panel=disable", "--config=disable", "--cache=none",
                                   "--address=" + env["IBUS_ADDRESS"]], env=env, stdout=subprocess.DEVNULL)
        bridge = subprocess.Popen([sys.executable, str(pathlib.Path(__file__).with_name("focus_bridge.py"))], env=env)
        try:
            deadline = time.monotonic() + 5
            while not (root / "bus").exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            os.environ["IBUS_ADDRESS"] = env["IBUS_ADDRESS"]
            IBus.init()
            bus = IBus.Bus()
            assert bus.is_connected(), "Isolated IBus did not start"
            pump(lambda: session.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", GLib.Variant("(s)", ("org.punto.Switcher.Focus",)), None, Gio.DBusCallFlags.NONE, 1000, None).unpack()[0])
            context = bus.create_input_context("gtk3-im:punto-test")
            context.set_capabilities(int(IBus.Capabilite.PREEDIT_TEXT | IBus.Capabilite.SURROUNDING_TEXT | IBus.Capabilite.FOCUS))
            committed = []
            deletes = []
            preedits = []
            context.connect("commit-text", lambda _, text: committed.append(text.get_text()))
            context.connect("delete-surrounding-text", lambda _, offset, count: deletes.append((offset, count)))
            context.connect("update-preedit-text", lambda _, text, cursor, visible: preedits.append(text.get_text()))
            pump(lambda: bus.name_has_owner("org.freedesktop.IBus.Punto"))
            assert any(e.get_name() == "punto-rs" for e in bus.list_engines()), "Component was not registered"
            assert bus.set_global_engine("punto-rs"), "Cannot select engine"
            context.focus_in()
            pump(lambda: context.get_engine() is not None and context.get_engine().get_name() == "punto-rs")
            context.set_surrounding_text(IBus.Text.new_from_string(""), 0, 0)
            for ch in "ghbdtn ":
                assert context.process_key_event(ord(ch), 0, 0), ch
            pump(lambda: bool(committed))
            assert committed == ["привет "], committed
            assert preedits and preedits[-1] == "", preedits

            context.set_surrounding_text(IBus.Text.new_from_string("привет "), 7, 7)
            assert context.process_key_event(0xFF14, 0, 0)  # ScrollLock: undo.
            pump(lambda: len(committed) == 2)
            assert deletes == [(-7, 7)], deletes
            assert committed[-1] == "ghbdtn ", committed

            context.set_surrounding_text(IBus.Text.new_from_string("ghbdtn"), 6, 0)
            assert context.process_key_event(ord("s"), 0, 4 | 8)
            pump(lambda: len(committed) == 3)
            assert committed[-1] == "привет", committed
            assert len(deletes) == 1, "Selected text must not be deleted twice"

            context.set_surrounding_text(IBus.Text.new_from_string("привет"), 6, 6)
            assert context.process_key_event(0xFF14, 0, 0)
            pump(lambda: len(committed) == 4)
            assert committed[-1] == "ghbdtn", committed

            for _ in range(30):
                context.set_content_type(IBus.InputPurpose.FREE_FORM, 0)
                context.set_content_type(IBus.InputPurpose.PASSWORD, 0)
                assert not context.process_key_event(ord("a"), 0, 0), "Password must pass through"
                assert not context.process_key_event(ord("s"), 0, 4 | 8), "Password shortcut must pass through"
            context.set_content_type(IBus.InputPurpose.FREE_FORM, 0)
            for ch in "hel":
                assert context.process_key_event(ord(ch), 0, 0)
            context.focus_out()
            pump(lambda: len(committed) == 5)
            assert committed[-1] == "hel", "Focus-out must commit preedit exactly once"
            def focus(app, title, locked=False):
                session.call_sync("org.punto.Switcher.Focus", "/org/punto/Switcher/Focus", "org.punto.Switcher.Focus", "SetFocus", GLib.Variant("(ssb)", (app, title, locked)), None, Gio.DBusCallFlags.NONE, 1000, None)
            focus("org.gnome.Terminal", "Terminal")
            context.focus_in()
            count = len(committed)
            for ch in "ghbdtn ":
                assert context.process_key_event(ord(ch), 0, 0)
            pump(lambda: len(committed) >= count + 7)
            assert "".join(committed[count:]) == "ghbdtn ", "Terminal exception failed"
            context.focus_out()
            focus("org.punto.TestEditor", "Editor", True)
            context.focus_in()
            assert not context.process_key_event(ord("a"), 0, 0), "Locked screen must pass through"
            context.focus_out()
            focus("org.punto.TestEditor", "Editor")
            if os.environ.get("PUNTO_TEST_GTK"):
                gtk_check()
            print("IBus integration: auto correction, preedit, selection, undo, password ordering, focus-out, exceptions, lock OK")
        finally:
            bridge.terminate()
            bridge.wait(timeout=5)
            if engine is not None:
                engine.terminate()
                try:
                    engine.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    engine.kill()
                    engine.wait()
            daemon.terminate()
            try:
                daemon.wait(timeout=5)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait()


if __name__ == "__main__":
    main()
