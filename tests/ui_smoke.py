#!/usr/bin/env python3
"""Launch settings on a disposable display and capture its first rendered frame."""
import os
import re
import ctypes
import pathlib
import subprocess
import sys
import tempfile
import time

import gi
gi.require_version("Gdk", "3.0")
gi.require_version("GdkX11", "3.0")
gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, GdkX11, Gio, GLib, Gtk

with tempfile.TemporaryDirectory(prefix="punto-ui-") as temp:
    env = os.environ.copy()
    env["WINIT_X11_SCALE_FACTOR"] = "1"
    env["XDG_CONFIG_HOME"] = temp
    env["IBUS_ADDRESS"] = "unix:path=" + temp + "/missing-ibus"
    subprocess.run(["setxkbmap", "-layout", "us,ru", "-option", "", "-option", "grp:ctrl_shift_toggle"], check=True)
    mapping_before = subprocess.check_output(["setxkbmap", "-query"])
    config_dir = pathlib.Path(temp) / "punto-rs"
    config_dir.mkdir()
    (config_dir / "config.toml").write_text('remember_layout = false\n[replacements]\nsig = "Best\\nRegards"\n')
    binary_dir = pathlib.Path(temp) / "bin"
    binary_dir.mkdir()
    (binary_dir / "punto-rs").symlink_to(pathlib.Path(sys.argv[1]).resolve())
    env["PATH"] = str(binary_dir) + os.pathsep + env["PATH"]
    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    def call(bus, path, interface, method, parameters=None):
        return connection.call_sync(bus, path, interface, method, parameters, None, Gio.DBusCallFlags.NONE, 1000, None).unpack()
    def owner(name):
        return call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", GLib.Variant("(s)", (name,)))[0]
    host = subprocess.Popen([sys.executable, str(pathlib.Path(__file__).with_name("tray_host.py"))])
    deadline = time.monotonic() + 5
    while not owner("org.kde.StatusNotifierWatcher"):
        assert time.monotonic() < deadline
        time.sleep(0.05)
    process = subprocess.Popen([sys.argv[1]], env=env)
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            assert process.poll() is None, "Settings exited before rendering"
            found = subprocess.run(["xdotool", "search", "--name", "Punto RS"], capture_output=True)
            if found.returncode == 0:
                break
            time.sleep(0.1)
        else:
            raise AssertionError("Settings window did not appear")
        time.sleep(1)
        assert owner("org.punto.Switcher"), "Background process did not start"
        first_owner = call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetNameOwner", GLib.Variant("(s)", ("org.punto.Switcher.Settings",)))
        duplicate = subprocess.run([sys.argv[1], "--settings"], env=env, capture_output=True, timeout=8)
        assert duplicate.returncode == 0, duplicate.stderr
        assert call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetNameOwner", GLib.Variant("(s)", ("org.punto.Switcher.Settings",))) == first_owner, "Second window replaced the first"
        print("Repeated launch presents the same settings instance OK", flush=True)
        items = call("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", ("org.kde.StatusNotifierWatcher", "RegisteredStatusNotifierItems")))[0]
        assert items, "No tray icon registered without IBus"
        tray_bus, _, tray_path = items[0].partition("/")
        tray_path = "/" + tray_path if tray_path else "/StatusNotifierItem"
        title = call(tray_bus, tray_path, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", ("org.kde.StatusNotifierItem", "Title")))[0]
        assert "Punto RS" in title, title
        icon_name = call(tray_bus, tray_path, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", ("org.kde.StatusNotifierItem", "IconName")))[0]
        icon_path = call(tray_bus, tray_path, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", ("org.kde.StatusNotifierItem", "IconThemePath")))[0]
        assert icon_name.startswith("punto-rs-"), "MATE requires a named icon"
        theme = Gtk.IconTheme.new()
        theme.append_search_path(icon_path)
        image = theme.load_icon(icon_name, 24, Gtk.IconLookupFlags.FORCE_SIZE)
        assert image and image.get_width() == 24, "Named tray icon cannot be loaded by GTK"
        print("Tray: Material icon resolves by name through GTK (MATE/Ayatana) OK")
        pixmaps = call(tray_bus, tray_path, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", ("org.kde.StatusNotifierItem", "IconPixmap")))[0]
        assert pixmaps and all(width > 0 and height > 0 and len(pixels) == width * height * 4 for width, height, pixels in pixmaps)
        assert call("org.punto.Switcher", "/org/punto/Switcher", "org.punto.Switcher", "InputReady")[0]
        assert not owner("org.freedesktop.IBus"), "Native startup launched IBus"
        window_id = found.stdout.decode().strip().splitlines()[-1]
        subprocess.run(["xdotool", "windowfocus", window_id], check=True)
        def click(x, y):
            subprocess.run(["xdotool", "mousemove", "--window", window_id, str(x), str(y), "click", "1"], check=True)
            time.sleep(0.2)
        settings_window = GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(), int(window_id))
        def save():
            file = config_dir / "config.toml"
            previous = file.stat().st_mtime_ns
            click(42, settings_window.get_height() - 24)
            deadline = time.monotonic() + 10
            while file.stat().st_mtime_ns == previous:
                assert time.monotonic() < deadline, "Save click was not processed"
                time.sleep(0.1)
            # Debug Hunspell reload blocks the UI; its RPC has a five-second deadline.
            time.sleep(5.2)
        autostart = pathlib.Path(temp) / "autostart/punto-rs.desktop"
        assert not autostart.exists(), "Autostart enabled without user choice"
        for enabled in (True, False, True):
            time.sleep(1)  # Allow the previous save/reload to finish before another click.
            click(166, 194)
            save()
            deadline = time.monotonic() + 5
            while autostart.exists() != enabled:
                assert time.monotonic() < deadline, "Autostart checkbox was not saved"
                time.sleep(0.1)
        time.sleep(1)
        assert "Exec=punto-rs --daemon\n" in autostart.read_text()
        print("Autostart: GUI checkbox saves enable/disable OK")
        def pump():
            deadline = time.monotonic() + 0.3
            context = GLib.MainContext.default()
            while time.monotonic() < deadline:
                while context.pending():
                    context.iteration(False)
                time.sleep(0.005)
        def key(*keys):
            subprocess.run(["xdotool", "key", "--clearmodifiers", *keys], check=True)
            pump()
        clipboard = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
        def paste(text):
            clipboard.set_text(text, -1)
            key("ctrl+a", "ctrl+v")
        def read_preview():
            key("ctrl+a", "ctrl+c")
            return clipboard.wait_for_text()
        def set_layout(language):
            call("org.punto.Switcher", "/org/punto/Switcher", "org.punto.Switcher", "SetLayout", GLib.Variant("(s)", (language,)))
            pump()
        xlib = ctypes.CDLL("libX11.so.6")
        xtest = ctypes.CDLL("libXtst.so.6")
        xlib.XOpenDisplay.restype = ctypes.c_void_p
        display = xlib.XOpenDisplay(None)
        xlib.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
        xlib.XKeysymToKeycode.restype = ctypes.c_ubyte
        xtest.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        xlib.XFlush.argtypes = [ctypes.c_void_p]
        def physical(text):
            for char in text:
                code = xlib.XKeysymToKeycode(display, ord(char))
                assert code
                for pressed in (True, False):
                    xtest.XTestFakeKeyEvent(display, code, pressed, 0)
                xlib.XFlush(display)
                pump()
        click(400, 390)
        set_layout("RU")
        physical("we we ")
        assert read_preview() == "we we ", "Short EN correction did not change the next physical keystrokes"
        set_layout("EN")
        paste("")
        physical("lf lf ")
        assert read_preview() == "да да ", "Short RU correction did not change the next physical keystrokes"
        print("Preview: short RU/EN words switch the layout for following physical input OK")
        for original in ("Ш", "ш", "«Ш»."):
            set_layout("RU")
            paste(original)
            key("space")
            expected = "«I». " if original == "«Ш»." else "I "
            assert read_preview() == expected, "English I was treated as an abbreviation"
            key("Right")
            physical("we ")
            assert read_preview() == expected + "we ", "I did not switch following input to English"
        paste("руддщ")
        key("space")
        assert read_preview() == "hello ", "RU preview did not correct on space"
        paste("руддщ ")
        assert read_preview() == "hello ", "Preview ignored a complete inserted word"
        paste("руддщ x")
        key("BackSpace")
        assert read_preview() == "руддщ ", "Deleting text unexpectedly triggered correction"
        paste("ghbdtn")
        key("space")
        assert read_preview() == "привет ", "EN preview did not correct on space"
        for original, expected in [
            ("ghjdthz.", "проверяю"),
            ("k.,k.", "люблю"),
            (";bpym", "жизнь"),
            ("[jxe", "хочу"),
            ("ghbdtn.", "привет."),
            ("(ghjdthz.),", "(проверяю),"),
            ('"ghbdtn"', '"привет"'),
            ("ghbdtn.com", "ghbdtn.com"),
        ]:
            paste(original)
            key("space")
            actual = read_preview()
            assert actual == expected + " ", (original, actual, expected)
        print("Preview punctuation: letters on punctuation keys, quotes, brackets and domains OK")
        paste("😀 ghbdtn world")
        key("Home", *(["Right"] * 8), "space")
        assert read_preview() == "😀 привет  world", "Preview damaged Unicode context or cursor"
        paste("sig")
        key("space")
        assert read_preview() == "Best\nRegards ", "Preview ignored saved snippets"
        paste("user@example.com")
        key("space")
        assert read_preview() == "user@example.com ", "Preview changed an email address"
        click(166, 110)
        click(400, 390)
        paste("руддщ")
        key("space")
        assert read_preview() == "руддщ ", "Preview ignored the automatic correction checkbox"
        click(166, 110)
        click(400, 390)
        paste("руддщ")
        key("space")
        assert read_preview() == "hello "
        key("Right")
        assert subprocess.check_output(["setxkbmap", "-query"]) == mapping_before
        print("Preview: RU/EN correction on space, Unicode cursor, snippets and disabled mode OK")
        settings_window = GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(), int(window_id))
        image = Gdk.pixbuf_get_from_window(settings_window, 0, 0, settings_window.get_width(), settings_window.get_height())
        assert image is not None
        image.savev(sys.argv[2], "png", [], [])
        runtime = call("org.punto.Switcher", "/org/punto/Switcher", "org.punto.Switcher", "RuntimeInfo")
        click(80, 59)  # Restart the background process, keeping this settings window.
        deadline = time.monotonic() + 10
        while True:
            try:
                current = call("org.punto.Switcher", "/org/punto/Switcher", "org.punto.Switcher", "RuntimeInfo")
                if current != runtime: break
            except GLib.Error:
                pass
            assert time.monotonic() < deadline, "Background restart did not finish"
            time.sleep(0.1)
        assert process.poll() is None
        print("Settings restart replaces the daemon without closing this window OK", flush=True)
        click(70, 104)
        image = Gdk.pixbuf_get_from_window(settings_window, 0, 0, settings_window.get_width(), settings_window.get_height())
        image.savev(str(pathlib.Path(sys.argv[2]).with_name("settings-hotkeys.png")), "png", [], [])
        config_file = config_dir / "config.toml"
        def saved_key():
            found = re.search(r'action = "toggle_auto"\nkeys = "([^"]*)"', config_file.read_text())
            return found.group(1) if found else "Ctrl+Alt+A"
        def wait_saved(expected):
            deadline = time.monotonic() + 8
            while saved_key() != expected:
                assert time.monotonic() < deadline, (saved_key(), expected)
                time.sleep(0.1)
            time.sleep(1)  # Finish the background reload before another edit.
        click(770, 153)  # Suggest a globally available shortcut for ToggleAuto.
        time.sleep(1)
        save()
        deadline = time.monotonic() + 8
        while saved_key() == "Ctrl+Alt+A":
            assert time.monotonic() < deadline, "Suggestion was not saved"
            time.sleep(0.1)
        assigned = saved_key()
        assert assigned.startswith("Ctrl+Super+F"), assigned
        wait_saved(assigned)
        # xdotool's Super alias differs with Ubuntu 26 keymaps; press physical modifier keycodes.
        symbols = [0xffe3, 0xffeb, 0xffbd + int(assigned.rsplit('F', 1)[1])]
        for symbol in symbols:
            xtest.XTestFakeKeyEvent(display, xlib.XKeysymToKeycode(display, symbol), True, 0)
        for symbol in reversed(symbols):
            xtest.XTestFakeKeyEvent(display, xlib.XKeysymToKeycode(display, symbol), False, 0)
        xlib.XFlush(display)
        pump()
        deadline = time.monotonic() + 5
        while 'automatic = false' not in config_file.read_text():
            assert time.monotonic() < deadline, ("Suggested global shortcut did not activate", assigned, call("org.punto.Switcher", "/org/punto/Switcher", "org.punto.Switcher", "Status"), config_file.read_text())
            time.sleep(0.1)
        def clear_part(x):
            click(x, 153)
        for x in (338, 527):
            clear_part(x)
        save()
        single = assigned.rsplit('+', 1)[1]
        wait_saved(single)
        clear_part(716)  # Empty key disables the action, still unsaved.
        click(430, 553)  # Revert unsaved hotkey edits.
        save()
        wait_saved(single)
        clear_part(716)
        save()
        wait_saved("")
        print("Hotkey UI: suggestion activates globally, empty modifiers allow one key, revert and disabled action save OK", flush=True)
        process.terminate()
        process.wait(timeout=5)
        assert owner("org.punto.Switcher"), "Closing settings killed the tray"
        print("GUI startup: settings rendered, native tray registered without IBus, tray survives window close OK")
        call("org.punto.Switcher", "/org/punto/Switcher", "org.punto.Switcher", "Quit")
        deadline = time.monotonic() + 5
        while owner("org.punto.Switcher"):
            assert time.monotonic() < deadline
            time.sleep(0.05)
        subprocess.run([sys.executable, "-c", "from gi.repository import Gio; import sys; app = Gio.DesktopAppInfo.new_from_filename(sys.argv[1]); assert app; assert app.launch([], None)", str(autostart)], env=env, check=True)
        deadline = time.monotonic() + 10
        while not owner("org.punto.Switcher"):
            assert time.monotonic() < deadline, "Autostart entry did not launch the daemon"
            time.sleep(0.05)
        assert not owner("org.punto.Switcher.Settings"), "Autostart opened a settings window"
        print("Autostart: desktop entry launches daemon without settings OK")
    finally:
        if 'settings_window' in locals() and process.poll() is None:
            image = Gdk.pixbuf_get_from_window(settings_window, 0, 0, settings_window.get_width(), settings_window.get_height())
            if image: image.savev(sys.argv[2], "png", [], [])
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        if owner("org.punto.Switcher"):
            call("org.punto.Switcher", "/org/punto/Switcher", "org.punto.Switcher", "Quit")
            deadline = time.monotonic() + 5
            while owner("org.punto.Switcher"):
                assert time.monotonic() < deadline, "Tray did not quit"
                time.sleep(0.05)
        host.terminate()
        host.wait(timeout=5)
