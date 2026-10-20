//! Общий X11-путь для приложений без AT-SPI. В памяти хранится только текущий фрагмент набора.
use crate::{
    config::Action,
    hotkey::Hotkey,
    layout::{self, Layout},
    service::Shared,
    x11::Keyboard,
};
use anyhow::{Context, Result, ensure};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xinput::{self, ConnectionExt as _},
        xkb::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
    },
    rust_connection::RustConnection,
};

enum Input {
    Key(xproto::KeyPressEvent),
    Reset,
    Barrier,
}

struct Recorder {
    control: RustConnection,
    window: u32,
    state: u16,
}
impl Recorder {
    fn new() -> Result<Self> {
        let (control, screen) = x11rb::connect(None)?;
        control.xinput_xi_query_version(2, 2)?.reply()?;
        ensure!(
            control.xkb_use_extension(1, 0)?.reply()?.supported,
            "XKB недоступен"
        );
        let root = control.setup().roots[screen].root;
        control
            .xinput_xi_select_events(
                root,
                &[xinput::EventMask {
                    deviceid: xinput::Device::ALL_MASTER.into(),
                    mask: vec![
                        xinput::XIEventMask::RAW_KEY_PRESS
                            | xinput::XIEventMask::RAW_KEY_RELEASE
                            | xinput::XIEventMask::RAW_BUTTON_PRESS,
                    ],
                }],
            )?
            .check()?;
        control
            .xkb_select_events(
                xkb::ID::USE_CORE_KBD.into(),
                0u16.into(),
                xkb::EventType::STATE_NOTIFY,
                0u16.into(),
                0u16.into(),
                &Default::default(),
            )?
            .check()?;
        let current = control
            .xkb_get_state(xkb::ID::USE_CORE_KBD.into())?
            .reply()?;
        let state = u16::from(current.mods) | (u16::from(u8::from(current.group)) << 13);
        let window = control.generate_id()?;
        control
            .create_window(
                0,
                window,
                root,
                0,
                0,
                1,
                1,
                0,
                xproto::WindowClass::INPUT_ONLY,
                0,
                &Default::default(),
            )?
            .check()?;
        Ok(Self {
            control,
            window,
            state,
        })
    }
    fn next(&mut self) -> Result<Option<Input>> {
        while let Some(event) = self.control.poll_for_event()? {
            match event {
                Event::XkbStateNotify(event) => {
                    self.state = u16::from(event.mods) | (u16::from(u8::from(event.group)) << 13)
                }
                Event::XinputRawKeyPress(event) | Event::XinputRawKeyRelease(event) => {
                    let focus = self.control.get_input_focus()?.reply()?.focus;
                    return Ok(Some(Input::Key(xproto::KeyPressEvent {
                        response_type: if event.event_type == xinput::RAW_KEY_PRESS_EVENT {
                            xproto::KEY_PRESS_EVENT
                        } else {
                            xproto::KEY_RELEASE_EVENT
                        },
                        detail: u8::try_from(event.detail)?,
                        time: event.time,
                        event: focus,
                        state: self.state.into(),
                        ..Default::default()
                    })));
                }
                Event::XinputRawButtonPress(_) => return Ok(Some(Input::Reset)),
                Event::ClientMessage(event) if event.window == self.window => {
                    return Ok(Some(Input::Barrier));
                }
                _ => {}
            }
        }
        Ok(None)
    }
    fn barrier(&self) -> Result<()> {
        let event =
            xproto::ClientMessageEvent::new(32, self.window, xproto::AtomEnum::WM_NAME, [0u32; 5]);
        self.control
            .send_event(false, self.window, xproto::EventMask::NO_EVENT, event)?
            .check()?;
        Ok(())
    }
}

struct Undo {
    original: String,
    replacement: String,
    layout: Layout,
}

pub struct InputBackend {
    recorder: Recorder,
    window: u32,
    buffer: String,
    last_word: Option<String>,
    pending: bool,
    released: bool,
    last_input: Instant,
    ignoring: bool,
    injected: VecDeque<(u8, bool)>,
    undo: Option<Undo>,
    pub active: bool,
    pub pointer_revision: u64,
    enter_word: Option<String>,
    enter_dirty: bool,
}
impl InputBackend {
    pub fn new() -> Result<Self> {
        Ok(Self {
            recorder: Recorder::new()?,
            window: 0,
            buffer: String::new(),
            last_word: None,
            pending: false,
            released: false,
            last_input: Instant::now(),
            ignoring: false,
            injected: VecDeque::new(),
            undo: None,
            active: false,
            pointer_revision: 0,
            enter_word: None,
            enter_dirty: false,
        })
    }
    pub(crate) fn clear(&mut self) {
        self.buffer.clear();
        self.last_word = None;
        self.pending = false;
        self.released = false;
        self.undo = None;
        self.enter_word = None;
        self.enter_dirty = false;
    }
    pub fn poll(
        &mut self,
        keyboard: &Keyboard,
        accessible_pid: Option<u32>,
        shared: &Shared,
    ) -> Result<()> {
        let enabled = {
            let state = shared.lock().unwrap();
            state.config.x11_unverified && !state.desktop.locked
        };
        let (window, pid, app, title) = keyboard.window_context()?;
        let accessible = pid.is_some() && pid == accessible_pid;
        if window != self.window || (!enabled && !accessible) {
            self.clear();
            self.window = window;
        }
        let was_active = self.active;
        self.active =
            enabled && !accessible && window > 1 && !app.to_ascii_lowercase().contains("punto-rs");
        if was_active && !self.active && !accessible {
            let mut state = shared.lock().unwrap();
            state.focused = false;
            state.engine.capabilities = 0;
        }
        if self.active {
            let mut state = shared.lock().unwrap();
            let crate::service::State { engine, config, .. } = &mut *state;
            let actual = keyboard.layout()?.unwrap_or(engine.layout);
            engine.layout = actual;
            engine.focus_in(&format!("x11:{window}"), &app, &title, config);
            if engine.layout != actual {
                keyboard.switch(engine.layout)?;
            }
            engine.capabilities = 1;
            state.focused = true;
            if state.message.starts_with("Ожидание доступного поля") {
                state.message = "X11: неизвестное поле; автозамена со звуком".into();
            }
        }
        let recording =
            self.active || (accessible && shared.lock().unwrap().engine.capabilities & 1 != 0);
        // Drain even when disabled, so a newly enabled backend never replays older typing.
        let mut drained = false;
        for _ in 0..256 {
            let Some(event) = self.recorder.next()? else {
                drained = true;
                break;
            };
            if matches!(event, Input::Reset) {
                self.pointer_revision = self.pointer_revision.wrapping_add(1);
            }
            if matches!(event, Input::Barrier) {
                self.ignoring = false;
                continue;
            }
            if let Input::Key(key) = &event
                && !self.injected.is_empty()
            {
                let actual = (
                    key.detail,
                    key.response_type & 127 == xproto::KEY_PRESS_EVENT,
                );
                if self.injected.front() == Some(&actual) {
                    self.injected.pop_front();
                } else {
                    self.clear();
                }
                continue;
            }
            if self.ignoring {
                continue;
            }
            if !recording {
                continue;
            }
            match event {
                Input::Reset => {
                    self.last_input = Instant::now();
                    self.clear();
                }
                Input::Key(key) => {
                    self.last_input = Instant::now();
                    // A queued event from a previously focused window must never edit the new one.
                    if key.event != window {
                        self.clear();
                        continue;
                    }
                    let mods = u16::from(key.state);
                    let Some(symbol) = keyboard.symbol(key.detail, mods) else {
                        self.clear();
                        continue;
                    };
                    if key.response_type & 127 == xproto::KEY_RELEASE_EVENT {
                        if symbol == 32 && self.pending {
                            self.released = true;
                        }
                        continue;
                    }
                    if matches!(symbol, 0xff0d | 0xff8d) {
                        self.enter_word = Some(std::mem::take(&mut self.buffer));
                        self.enter_dirty = false;
                        self.pending = false;
                        self.undo = None;
                        self.last_word = None;
                        continue;
                    }
                    if self.enter_word.is_some() {
                        self.enter_dirty = true;
                    }
                    if (0xffe1..=0xffee).contains(&symbol) {
                        continue;
                    }
                    let is_hotkey = shared.lock().unwrap().config.hotkeys.iter().any(|binding| {
                        Hotkey::parse(&binding.keys)
                            .is_ok_and(|hotkey| keyboard.matches_hotkey(key.detail, mods, hotkey))
                    });
                    if is_hotkey {
                        continue;
                    }
                    self.undo = None;
                    self.last_word = None;
                    if mods & 0xcc != 0 {
                        self.clear();
                        continue;
                    }
                    if symbol == 0xff08 {
                        self.buffer.pop();
                        self.pending = false;
                        continue;
                    }
                    let Some(ch) = xkeysym::Keysym::new(symbol)
                        .key_char()
                        .filter(|ch| !ch.is_control())
                    else {
                        self.clear();
                        continue;
                    };
                    if self.pending {
                        self.buffer.clear();
                        self.pending = false;
                        self.released = false;
                    }
                    self.buffer.push(ch);
                    if self.buffer.len() > 1024 {
                        self.clear();
                    }
                    if ch == ' ' {
                        self.pending = true;
                    } else if layout::delimiter(ch) {
                        self.buffer.clear();
                    }
                }
                Input::Barrier => unreachable!(),
            }
        }
        if drained
            && !self.injected.is_empty()
            && self.last_input.elapsed() > Duration::from_secs(1)
        {
            self.injected.clear();
            self.clear();
            shared.lock().unwrap().message =
                "X11: не получены все события замены; откат сброшен".into();
        }
        // shortcut: 25 ms avoids overwriting queued typing; serialize physical input if shorter pauses must be supported.
        if drained
            && self.active
            && self.pending
            && self.released
            && self.injected.is_empty()
            && self.last_input.elapsed() >= Duration::from_millis(25)
            && keyboard.idle().is_ok()
        {
            self.pending = false;
            self.released = false;
            self.correct(keyboard, shared)?;
        }
        Ok(())
    }
    pub fn take_enter_word(&mut self) -> Option<String> {
        let word = self
            .enter_word
            .take()
            .or_else(|| Some(std::mem::take(&mut self.buffer)));
        if std::mem::take(&mut self.enter_dirty) {
            None
        } else {
            word
        }
    }
    fn correct(&mut self, keyboard: &Keyboard, shared: &Shared) -> Result<()> {
        let original = std::mem::take(&mut self.buffer);
        let Some(word) = original.strip_suffix(' ').filter(|word| !word.is_empty()) else {
            return Ok(());
        };
        self.last_word = Some(original.clone());
        let old_layout = keyboard.layout()?.context("Неизвестная раскладка")?;
        let correction = {
            let state = shared.lock().unwrap();
            if !state.engine.automatic(&state.config) {
                return Ok(());
            }
            state.dictionaries.replacement(
                word,
                layout::word_layout(word, old_layout),
                &state.config,
            )
        };
        if let Some(correction) = correction {
            let replacement = correction.text + " ";
            if replacement != original {
                self.replace(
                    keyboard,
                    original.chars().count(),
                    &replacement,
                    correction.layout,
                )?;
                self.last_word = Some(replacement.clone());
                self.undo = Some(Undo {
                    original,
                    replacement,
                    layout: old_layout,
                });
                shared.lock().unwrap().engine.layout = correction.layout;
                crate::service::play_sound(shared.clone());
            }
        }
        Ok(())
    }
    fn replace(
        &mut self,
        keyboard: &Keyboard,
        count: usize,
        text: &str,
        layout: Layout,
    ) -> Result<()> {
        // shortcut: X11 cannot identify hidden fields without accessibility; unknown fields use the configurable X11 fallback.
        match keyboard.replace_tail(self.window, count, text, layout) {
            Ok(injected) => {
                self.last_input = Instant::now();
                self.injected = injected.into();
                Ok(())
            }
            Err(error) => {
                self.clear();
                self.ignoring = true;
                self.recorder.barrier()?;
                Err(error)
            }
        }
    }
    pub fn action(&mut self, action: Action, keyboard: &Keyboard, shared: &Shared) -> Result<()> {
        ensure!(
            self.active && keyboard.input_focus()? == self.window,
            "Фокус изменился"
        );
        let old_layout = keyboard.layout()?.context("Неизвестная раскладка")?;
        if action == Action::Undo {
            let undo = self.undo.take().context("Нет преобразования для отмены")?;
            self.replace(
                keyboard,
                undo.replacement.chars().count(),
                &undo.original,
                undo.layout,
            )?;
            self.last_word = Some(undo.original);
            shared.lock().unwrap().engine.layout = undo.layout;
            return Ok(());
        }
        let (original, count) = if action == Action::ConvertWord {
            let original = if self.buffer.is_empty() {
                self.last_word.take().unwrap_or_default()
            } else {
                std::mem::take(&mut self.buffer)
            };
            ensure!(!original.is_empty(), "Нет набранного слова");
            let count = original.chars().count();
            (original, count)
        } else {
            (self.selection(keyboard)?, 0)
        };
        let replacement = match action {
            Action::ChangeCase => {
                if original
                    .chars()
                    .filter(|ch| ch.is_alphabetic())
                    .all(char::is_uppercase)
                {
                    original.to_lowercase()
                } else {
                    original.to_uppercase()
                }
            }
            Action::Transliterate => layout::transliterate(&original),
            _ => layout::convert_selection(&original),
        };
        if replacement == original {
            return Ok(());
        }
        keyboard.typing_plan(&original, old_layout)?;
        let layout = if matches!(action, Action::ConvertSelection | Action::ConvertWord) {
            layout::word_layout(&replacement, old_layout)
        } else {
            old_layout
        };
        self.replace(keyboard, count, &replacement, layout)?;
        self.last_word = Some(replacement.clone());
        self.undo = Some(Undo {
            original,
            replacement,
            layout: old_layout,
        });
        shared.lock().unwrap().engine.layout = layout;
        Ok(())
    }
    fn selection(&mut self, keyboard: &Keyboard) -> Result<String> {
        let connection = &self.recorder.control;
        let owner = connection
            .get_selection_owner(xproto::AtomEnum::PRIMARY.into())?
            .reply()?
            .owner;
        ensure!(owner != 0, "Нет выделенного текста PRIMARY");
        let utf8 = connection.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
        let property = connection
            .intern_atom(false, b"PUNTO_SELECTION")?
            .reply()?
            .atom;
        connection
            .delete_property(self.recorder.window, property)?
            .check()?;
        connection
            .convert_selection(
                self.recorder.window,
                xproto::AtomEnum::PRIMARY.into(),
                utf8,
                property,
                0u32,
            )?
            .check()?;
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            let event = connection.poll_for_event()?;
            match &event {
                Some(Event::XinputRawKeyPress(_) | Event::XinputRawButtonPress(_)) => {
                    anyhow::bail!("Ввод изменился во время чтения выделения")
                }
                Some(Event::XkbStateNotify(event)) => {
                    self.recorder.state =
                        u16::from(event.mods) | (u16::from(u8::from(event.group)) << 13)
                }
                _ => {}
            }
            if let Some(Event::SelectionNotify(event)) = event {
                ensure!(
                    event.property == property,
                    "Приложение не предоставило выделение UTF-8"
                );
                let reply = connection
                    .get_property(true, self.recorder.window, property, utf8, 0, 16384)?
                    .reply()?;
                ensure!(
                    reply.type_ == utf8 && reply.format == 8 && reply.bytes_after == 0,
                    "Неподдерживаемое выделение"
                );
                ensure!(
                    keyboard.input_focus()? == self.window
                        && connection
                            .get_selection_owner(xproto::AtomEnum::PRIMARY.into())?
                            .reply()?
                            .owner
                            == owner,
                    "Выделение или фокус изменились"
                );
                let text = String::from_utf8(reply.value)?;
                ensure!(!text.is_empty(), "Пустое выделение");
                return Ok(text);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        anyhow::bail!("Приложение не ответило на запрос выделения")
    }
}
