const BUS_NAME = 'org.punto.Switcher.Focus';
const OBJECT_PATH = '/org/punto/Switcher/Focus';
const XML = `<node><interface name="org.punto.Switcher.Focus"><method name="GetFocus"><arg direction="out" type="s" name="app"/><arg direction="out" type="s" name="title"/><arg direction="out" type="b" name="locked"/></method><method name="GetLayout"><arg direction="out" type="s"/></method><method name="SetLayout"><arg direction="in" type="s"/><arg direction="out" type="b"/></method></interface></node>`;

class FocusBridge {
    enable() {
        this._object = Gio.DBusExportedObject.wrapJSObject(XML, this);
        this._object.export(Gio.DBus.session, OBJECT_PATH);
        this._owner = Gio.bus_own_name_on_connection(Gio.DBus.session, BUS_NAME, Gio.BusNameOwnerFlags.NONE, null, null);
    }

    GetFocus() {
        const locked = Main.sessionMode.isLocked || Main.sessionMode.isGreeter;
        const window = global.display.focus_window;
        if (locked || !window)
            return ['', '', locked];
        const app = Shell.WindowTracker.get_default().get_window_app(window);
        return [app?.get_id() ?? window.get_wm_class() ?? '', window.get_title() ?? '', false];
    }

    GetLayout() {
        if (Main.sessionMode.isLocked || Main.sessionMode.isGreeter) return '';
        const source = InputSourceManager.getInputSourceManager().currentSource;
        return source?.type === 'xkb' ? source.id : '';
    }

    SetLayout(layout) {
        if (!['us', 'ru'].includes(layout) || Main.sessionMode.isLocked || Main.sessionMode.isGreeter) return false;
        const manager = InputSourceManager.getInputSourceManager();
        const source = Object.values(manager.inputSources).find(source => source.type === 'xkb' && source.id.split('+')[0] === layout);
        if (!source) return false;
        source.activate(false);
        return true;
    }

    disable() {
        if (this._owner) Gio.bus_unown_name(this._owner);
        this._owner = null;
        this._object?.unexport();
        this._object = null;
    }
}
