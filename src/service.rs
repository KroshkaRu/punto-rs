//! Службы D-Bus: добровольный движок IBus и управление фоновым процессом из GUI.

use crate::{
    config::Config,
    desktop::Focus,
    dictionary::Dictionaries,
    engine::{Effect, Engine, Surrounding},
};
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
};
use zbus::{
    interface,
    object_server::SignalEmitter,
    zvariant::{OwnedObjectPath, OwnedValue, Structure, Value},
};

pub const NAME: &str = "org.freedesktop.IBus.Punto";
pub const ENGINE_PATH: &str = "/org/freedesktop/IBus/Engine/Punto";
pub type Shared = Arc<Mutex<State>>;

pub struct State {
    pub config: Config,
    pub path: PathBuf,
    pub dictionaries: Arc<Dictionaries>,
    pub engine: Engine,
    pub desktop: Focus,
    pub message: String,
    pub connected: bool,
    pub focused: bool,
    pub backend: String,
    pub actions: std::collections::VecDeque<crate::config::Action>,
    pub connect_shortcuts: bool,
    pub active_hotkeys: Vec<String>,
    pub rebind_hotkeys: bool,
}

impl State {
    pub fn automatic(&self) -> bool {
        self.connected && self.focused && self.engine.automatic(&self.config)
    }
    pub fn new(config: Config, path: PathBuf, dictionaries: Dictionaries) -> Self {
        Self {
            engine: Engine::new(&config),
            config,
            path,
            dictionaries: Arc::new(dictionaries),
            desktop: Focus::default(),
            message: String::new(),
            connected: false,
            focused: false,
            backend: String::new(),
            actions: std::collections::VecDeque::new(),
            connect_shortcuts: false,
            active_hotkeys: vec![],
            rebind_hotkeys: false,
        }
    }
}

fn attachments() -> HashMap<String, OwnedValue> {
    HashMap::new()
}
fn owned(value: impl Into<Value<'static>>) -> OwnedValue {
    value.into().try_to_owned().expect("owned D-Bus value")
}

pub fn text_value(text: &str) -> OwnedValue {
    let attrs = owned(Structure::from((
        "IBusAttrList".to_string(),
        attachments(),
        Vec::<OwnedValue>::new(),
    )));
    owned(Structure::from((
        "IBusText".to_string(),
        attachments(),
        text.to_string(),
        attrs,
    )))
}

pub fn decode_text(value: OwnedValue) -> Option<String> {
    let structure = Structure::try_from(value).ok()?;
    if structure.signature().to_string() != "(sa{sv}sv)" {
        return None;
    }
    if <&str>::try_from(&structure.fields()[0]).ok()? != "IBusText" {
        return None;
    }
    Some(<&str>::try_from(&structure.fields()[2]).ok()?.to_string())
}

struct Factory;

struct EngineService {
    shared: Shared,
}

struct EngineProperties {
    shared: Shared,
}

// Properties.Set must share the key-event queue so a password hint cannot arrive late.
#[interface(name = "org.freedesktop.DBus.Properties", spawn = false)]
impl EngineProperties {
    fn get(&self, interface: &str, property: &str) -> zbus::fdo::Result<OwnedValue> {
        if interface == "org.freedesktop.IBus.Engine"
            && matches!(property, "FocusId" | "ActiveSurroundingText")
        {
            Ok(OwnedValue::from(true))
        } else {
            Err(zbus::fdo::Error::UnknownProperty(property.into()))
        }
    }
    fn get_all(&self, interface: &str) -> HashMap<String, OwnedValue> {
        if interface != "org.freedesktop.IBus.Engine" {
            return HashMap::new();
        }
        ["FocusId", "ActiveSurroundingText"]
            .into_iter()
            .map(|name| (name.to_string(), OwnedValue::from(true)))
            .collect()
    }
    fn set(&self, interface: &str, property: &str, value: OwnedValue) -> zbus::fdo::Result<()> {
        if interface != "org.freedesktop.IBus.Engine" || property != "ContentType" {
            return Err(zbus::fdo::Error::UnknownProperty(property.into()));
        }
        let structure =
            Structure::try_from(value).map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        if structure.signature().to_string() != "(uu)" {
            return Err(zbus::fdo::Error::InvalidArgs("Expected (uu)".into()));
        }
        let purpose = u32::try_from(&structure.fields()[0])
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        let hints = u32::try_from(&structure.fields()[1])
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        let mut state = self.shared.lock().unwrap();
        state.engine.purpose = purpose;
        state.engine.hints = hints;
        if state.engine.sensitive() {
            state.engine.clear();
        }
        Ok(())
    }
}
#[interface(name = "org.freedesktop.IBus.Service", spawn = false)]
impl EngineService {
    fn destroy(&self) {
        self.shared.lock().unwrap().engine.clear();
    }
}

#[interface(name = "org.freedesktop.IBus.Factory")]
impl Factory {
    fn create_engine(&self, name: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        if name != "punto-rs" {
            return Err(zbus::fdo::Error::InvalidArgs("Unknown engine".into()));
        }
        Ok(OwnedObjectPath::try_from(ENGINE_PATH).expect("static object path"))
    }
}

pub struct IBusEngine {
    pub shared: Shared,
}

impl IBusEngine {
    async fn effects(
        &self,
        effects: Vec<Effect>,
        emitter: &SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        for effect in effects {
            let result = match effect {
                Effect::Commit(text) => Self::commit_text(emitter, text_value(&text)).await,
                Effect::Preedit(text) => {
                    Self::update_preedit_text(
                        emitter,
                        text_value(&text),
                        text.chars().count() as u32,
                        !text.is_empty(),
                        1,
                    )
                    .await
                }
                Effect::Delete { offset, count } => {
                    Self::delete_surrounding_text(emitter, offset, count).await
                }
                Effect::Message(message) => {
                    self.shared.lock().unwrap().message = message;
                    Ok(())
                }
                Effect::Bell => {
                    play_sound(self.shared.clone());
                    Ok(())
                }
            };
            result.map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        }
        Ok(())
    }
}

#[interface(name = "org.freedesktop.IBus.Engine", spawn = false)]
impl IBusEngine {
    async fn process_key_event(
        &self,
        keyval: u32,
        _keycode: u32,
        modifiers: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<bool> {
        let (handled, effects) = {
            let mut shared = self.shared.lock().unwrap();
            if shared.desktop.locked {
                return Ok(false);
            }
            let automatic = shared.config.automatic;
            let State {
                engine,
                config,
                dictionaries,
                ..
            } = &mut *shared;
            let result = engine.process(keyval, modifiers, config, dictionaries);
            if shared.config.automatic != automatic
                && let Err(error) = shared.config.save(&shared.path)
            {
                shared.message = format!("Сохранение настроек: {error}");
            }
            result
        };
        self.effects(effects, &emitter).await?;
        Ok(handled)
    }
    fn set_capabilities(&self, capabilities: u32) {
        self.shared.lock().unwrap().engine.capabilities = capabilities;
    }
    async fn focus_in(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.focus_in_id("legacy", "", emitter).await
    }
    async fn focus_in_id(
        &self,
        path: &str,
        client: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        // Read at the focus boundary: a cached window can belong to the previous app.
        let focus = blocking::unblock(crate::desktop::read).await;
        {
            let mut shared = self.shared.lock().unwrap();
            shared.desktop = focus.clone();
            let app = if focus.known {
                focus.app
            } else {
                client
                    .split_once(':')
                    .map(|(_, app)| app)
                    .unwrap_or(client)
                    .to_string()
            };
            shared.focused = client != "fake";
            let State { engine, config, .. } = &mut *shared;
            engine.focus_in(path, &app, &focus.title, config);
            let unknown = !focus.known
                && matches!(
                    client,
                    "gnome-shell" | "QIBusInputContext" | "Qt" | "xim" | ""
                );
            if unknown {
                engine.excluded = true;
                shared.message = "Не определено приложение: автокоррекция отключена. В GNOME Wayland включите мост Punto RS".into();
            }
        }
        Self::require_surrounding_text(&emitter)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
    async fn focus_out(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        // PREEDIT_COMMIT mode makes the client commit its preedit on focus-out.
        let effects = {
            let mut state = self.shared.lock().unwrap();
            state.focused = false;
            state.engine.clear()
        };
        self.effects(effects, &emitter).await
    }
    async fn focus_out_id(
        &self,
        _path: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.focus_out(emitter).await
    }
    async fn reset(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let effects = {
            let mut state = self.shared.lock().unwrap();
            let effects = state.engine.flush();
            state.engine.clear();
            effects
        };
        self.effects(effects, &emitter).await
    }
    fn enable(&self) {
        self.shared.lock().unwrap().connected = true;
    }
    async fn disable(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.focus_out(emitter).await
    }
    fn set_surrounding_text(&self, text: OwnedValue, cursor: u32, anchor: u32) {
        if let Some(text) = decode_text(text) {
            self.shared
                .lock()
                .unwrap()
                .engine
                .set_surrounding(Surrounding {
                    text,
                    cursor: cursor as usize,
                    anchor: anchor as usize,
                });
        }
    }
    #[zbus(property)]
    fn focus_id(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn active_surrounding_text(&self) -> bool {
        true
    }
    fn set_cursor_location(&self, _x: i32, _y: i32, _width: i32, _height: i32) {}
    fn property_activate(&self, _name: &str, _state: u32) {}
    fn property_show(&self, _name: &str) {}
    fn property_hide(&self, _name: &str) {}
    fn candidate_clicked(&self, _index: u32, _button: u32, _state: u32) {}
    fn page_up(&self) {}
    fn page_down(&self) {}
    fn cursor_up(&self) {}
    fn cursor_down(&self) {}
    #[zbus(signal)]
    async fn commit_text(emitter: &SignalEmitter<'_>, text: OwnedValue) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn update_preedit_text(
        emitter: &SignalEmitter<'_>,
        text: OwnedValue,
        cursor: u32,
        visible: bool,
        mode: u32,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn delete_surrounding_text(
        emitter: &SignalEmitter<'_>,
        offset: i32,
        count: u32,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn require_surrounding_text(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

#[derive(Clone)]
pub struct Control {
    pub shared: Shared,
    pub show: Arc<std::sync::atomic::AtomicBool>,
    pub quit: Arc<std::sync::atomic::AtomicBool>,
}

#[interface(name = "org.punto.Switcher")]
impl Control {
    fn runtime_info(&self) -> (String, u32) {
        (env!("CARGO_PKG_VERSION").into(), std::process::id())
    }
    fn hotkey_available(&self, keys: &str) -> zbus::fdo::Result<bool> {
        let key = crate::hotkey::Hotkey::parse(keys)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        if self
            .shared
            .lock()
            .unwrap()
            .active_hotkeys
            .iter()
            .any(|text| crate::hotkey::Hotkey::parse(text).ok() == Some(key))
        {
            return Ok(true);
        }
        if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") {
            return Err(zbus::fdo::Error::NotSupported(
                "На Wayland занятость определяет портал".into(),
            ));
        }
        let config = Config {
            hotkeys: vec![crate::config::Binding {
                action: crate::config::Action::SwitchLayout,
                keys: keys.into(),
            }],
            ..Config::default()
        };
        crate::x11::Keyboard::new()
            .and_then(|mut keyboard| keyboard.bind(&config))
            .map(|failures| failures.is_empty())
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }
    fn set_layout(&self, language: &str) -> zbus::fdo::Result<()> {
        let layout = match language {
            "EN" => crate::layout::Layout::En,
            "RU" => crate::layout::Layout::Ru,
            _ => return Err(zbus::fdo::Error::InvalidArgs("Ожидается EN или RU".into())),
        };
        let mut state = self.shared.lock().unwrap();
        if state.desktop.locked {
            return Err(zbus::fdo::Error::Failed("Экран заблокирован".into()));
        }
        let result = (|| -> Result<()> {
            if state.backend != "IBus" {
                if std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland")
                    && std::env::var_os("DISPLAY").is_some()
                {
                    crate::x11::Keyboard::new()?.switch(layout)?;
                } else {
                    crate::session_layout::SessionLayout::discover()?
                        .context("Управление раскладкой рабочего стола недоступно")?
                        .switch(layout)?;
                }
            }
            Ok(())
        })();
        result.map_err(|error| zbus::fdo::Error::Failed(format!("{error:#}")))?;
        state.engine.layout = layout;
        Ok(())
    }
    fn connect_shortcuts(&self) {
        self.shared.lock().unwrap().connect_shortcuts = true;
    }
    fn input_backend(&self) -> String {
        self.shared.lock().unwrap().backend.clone()
    }
    fn activate_action(&self, id: &str) -> zbus::fdo::Result<()> {
        let action = crate::config::Action::from_id(id)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("Неизвестное действие".into()))?;
        let mut state = self.shared.lock().unwrap();
        if state.backend == "IBus" {
            return Err(zbus::fdo::Error::NotSupported(
                "В IBus используйте горячие клавиши в поле ввода".into(),
            ));
        }
        if state.actions.len() < 16 {
            state.actions.push_back(action);
        }
        Ok(())
    }
    fn input_ready(&self) -> bool {
        self.shared.lock().unwrap().connected
    }
    fn quit(&self) {
        self.quit.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    fn show_settings(&self) {
        self.show.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    fn status(&self) -> (String, bool, String, String) {
        let state = self.shared.lock().unwrap();
        (
            state.engine.layout.label().into(),
            state.automatic(),
            state.engine.app.clone(),
            state.message.clone(),
        )
    }
    fn reload(&self) -> zbus::fdo::Result<()> {
        let path = self.shared.lock().unwrap().path.clone();
        let config = Config::load(&path).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let dictionaries =
            Dictionaries::load(&config).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let mut state = self.shared.lock().unwrap();
        state.config = config;
        state.rebind_hotkeys = true;
        state.dictionaries = Arc::new(dictionaries);
        let focus = state.engine.focus.clone();
        let app = state.engine.app.clone();
        let title = state.engine.title.clone();
        let State { engine, config, .. } = &mut *state;
        engine.focus_in(&focus, &app, &title, config);
        Ok(())
    }
}

pub fn connect_ibus(shared: Shared) -> Result<zbus::blocking::Connection> {
    let address = std::env::var("IBUS_ADDRESS")
        .ok()
        .filter(|a| !a.is_empty())
        .unwrap_or_else(|| {
            Command::new("ibus")
                .arg("address")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default()
        });
    anyhow::ensure!(
        !address.is_empty() && address != "(null)",
        "IBus не запущен; включите IBus и войдите в сеанс заново"
    );
    let connection = zbus::blocking::connection::Builder::address(address.as_str())?
        .serve_at("/org/freedesktop/IBus/Factory", Factory)?
        .serve_at(
            ENGINE_PATH,
            IBusEngine {
                shared: shared.clone(),
            },
        )?
        .serve_at(
            ENGINE_PATH,
            EngineService {
                shared: shared.clone(),
            },
        )?
        .build()
        .context("Не удалось подключиться к IBus")?;
    connection
        .object_server()
        .remove::<zbus::fdo::Properties, _>(ENGINE_PATH)?;
    connection.object_server().at(
        ENGINE_PATH,
        EngineProperties {
            shared: shared.clone(),
        },
    )?;
    connection.request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())?;
    shared.lock().unwrap().connected = true;
    Ok(connection)
}

pub fn control_connection(control: Control) -> Result<zbus::blocking::Connection> {
    Ok(zbus::blocking::connection::Builder::session()?
        .serve_at("/org/punto/Switcher", control)?
        .allow_name_replacements(false)
        .replace_existing_names(false)
        .name("org.punto.Switcher")?
        .build()?)
}

pub fn running() -> bool {
    zbus::blocking::Connection::session()
        .ok()
        .and_then(|connection| {
            zbus::blocking::fdo::DBusProxy::new(&connection)
                .ok()?
                .name_has_owner("org.punto.Switcher".try_into().ok()?)
                .ok()
        })
        .unwrap_or(false)
}

pub(crate) fn play_sound(shared: Shared) {
    std::thread::spawn(move || {
        let result = Command::new("canberra-gtk-play")
            .args(["-i", "complete"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !result.is_ok_and(|status| status.success()) {
            shared.lock().unwrap().message =
                "Звук недоступен; установите libcanberra-gtk3-bin и тему звуков".into();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_wire_format_roundtrip() {
        let value = text_value("Привет 😀");
        assert_eq!(value.value_signature().to_string(), "(sa{sv}sv)");
        assert_eq!(decode_text(value).as_deref(), Some("Привет 😀"));
        assert!(decode_text(OwnedValue::from(5u32)).is_none());
    }
}
