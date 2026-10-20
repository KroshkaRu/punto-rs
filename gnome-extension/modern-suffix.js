export default class PuntoFocusExtension extends Extension {
    enable() { this._bridge = new FocusBridge(); this._bridge.enable(); }
    disable() { this._bridge?.disable(); this._bridge = null; }
}
