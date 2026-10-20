//! Существующие группы XKB и отдельные горячие клавиши X11; системная карта клавиатуры не заменяется.

use crate::{
    config::{Action, Config},
    hotkey::Hotkey,
    layout::Layout,
};
use anyhow::{Context, Result};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xkb::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

pub struct Keyboard {
    connection: RustConnection,
    root: u32,
    groups: Vec<String>,
    keys: Vec<u32>,
    first: u8,
    width: usize,
    bindings: Vec<(Action, String)>,
    grabs: Vec<(u8, u16, Action)>,
    pub reported: Option<Layout>,
    failures: Vec<String>,
    bindings_valid: bool,
    symbols_map: Vec<xkb::KeySymMap>,
    symbols_first: u8,
    enter_grabs: Vec<(u8, u16)>,
    enter: Option<xproto::KeyPressEvent>,
}

impl Keyboard {
    pub fn new() -> Result<Self> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        anyhow::ensure!(
            connection.xkb_use_extension(1, 0)?.reply()?.supported,
            "XKB недоступен"
        );
        let mut keyboard = Self {
            connection,
            root,
            groups: vec![],
            keys: vec![],
            first: 0,
            width: 0,
            bindings: vec![],
            grabs: vec![],
            reported: None,
            failures: vec![],
            bindings_valid: false,
            symbols_map: vec![],
            symbols_first: 0,
            enter_grabs: vec![],
            enter: None,
        };
        keyboard.mapping()?;
        Ok(keyboard)
    }
    fn mapping(&mut self) -> Result<()> {
        let atom = self
            .connection
            .intern_atom(false, b"_XKB_RULES_NAMES")?
            .reply()?
            .atom;
        let names = self
            .connection
            .get_property(false, self.root, atom, xproto::AtomEnum::STRING, 0, 4096)?
            .reply()?
            .value;
        self.groups = names
            .split(|b| *b == 0)
            .nth(2)
            .map(|names| {
                String::from_utf8_lossy(names)
                    .split(',')
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let setup = self.connection.setup();
        self.first = setup.min_keycode;
        let map = self
            .connection
            .get_keyboard_mapping(self.first, setup.max_keycode - self.first + 1)?
            .reply()?;
        self.width = map.keysyms_per_keycode as usize;
        self.keys = map.keysyms;
        let map = self
            .connection
            .xkb_get_map(
                xkb::ID::USE_CORE_KBD.into(),
                xkb::MapPart::KEY_SYMS,
                0u16.into(),
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0u16.into(),
                0,
                0,
                0,
                0,
                0,
                0,
            )?
            .reply()?;
        self.symbols_first = map.first_key_sym;
        self.symbols_map = map.map.syms_rtrn.context("XKB не вернул символы клавиш")?;
        Ok(())
    }
    pub fn matches_hotkey(&self, code: u8, state: u16, hotkey: Hotkey) -> bool {
        self.symbols(code).contains(&hotkey.key) && hotkey.modifiers as u16 == state & 0xcd
    }
    pub fn symbol(&self, code: u8, state: u16) -> Option<u32> {
        let key = self
            .symbols_map
            .get(usize::from(code.checked_sub(self.symbols_first)?))?;
        let group = usize::from((state >> 13) & 3);
        let group = if group >= usize::from(key.group_info & 15) {
            0
        } else {
            group
        };
        let shift = usize::from(state & 1 != 0);
        let base = *key.syms.get(group * usize::from(key.width))?;
        let shifted = key
            .syms
            .get(group * usize::from(key.width) + 1)
            .copied()
            .filter(|s| *s != 0)
            .unwrap_or(base);
        let caps = state & 2 != 0
            && xkeysym::Keysym::new(base)
                .key_char()
                .is_some_and(char::is_alphabetic);
        Some(if (shift != 0) ^ caps { shifted } else { base })
    }
    pub fn window_context(&self) -> Result<(u32, Option<u32>, String, String)> {
        let focus = self.input_focus()?;
        let pid_atom = self
            .connection
            .intern_atom(false, b"_NET_WM_PID")?
            .reply()?
            .atom;
        let mut window = focus;
        let mut pid = None;
        let mut app = String::new();
        let mut title = String::new();
        for _ in 0..32 {
            if window <= 1 || window == self.root {
                break;
            }
            if pid.is_none() {
                pid = self
                    .connection
                    .get_property(false, window, pid_atom, xproto::AtomEnum::CARDINAL, 0, 1)?
                    .reply()?
                    .value32()
                    .and_then(|mut values| values.next());
            }
            if app.is_empty() {
                let value = self
                    .connection
                    .get_property(
                        false,
                        window,
                        xproto::AtomEnum::WM_CLASS,
                        xproto::AtomEnum::STRING,
                        0,
                        1024,
                    )?
                    .reply()?
                    .value;
                app = String::from_utf8_lossy(&value).replace('\0', " ");
            }
            if title.is_empty() {
                let value = self
                    .connection
                    .get_property(
                        false,
                        window,
                        xproto::AtomEnum::WM_NAME,
                        xproto::AtomEnum::ANY,
                        0,
                        1024,
                    )?
                    .reply()?
                    .value;
                title = String::from_utf8_lossy(&value).into_owned();
            }
            window = self.connection.query_tree(window)?.reply()?.parent;
        }
        Ok((focus, pid, app, title))
    }
    /// Batch keyboard edits without changing the clipboard or keymap.
    pub fn replace_tail(
        &self,
        focus: u32,
        count: usize,
        text: &str,
        layout: Layout,
    ) -> Result<Vec<(u8, bool)>> {
        anyhow::ensure!(
            count <= 4096,
            "Слишком большой фрагмент для клавиатурной замены"
        );
        anyhow::ensure!(
            self.groups.iter().any(|name| match layout {
                Layout::En => matches!(name.as_str(), "us" | "gb"),
                Layout::Ru => name == "ru",
            }),
            "Нужная раскладка не настроена в системе"
        );
        let plan = self.typing_plan(text, layout)?;
        let backspace = self
            .keys
            .chunks(self.width)
            .position(|keys| keys.contains(&0xff08))
            .context("Нет Backspace")? as u8
            + self.first;
        let shift = self
            .keys
            .chunks(self.width)
            .position(|keys| keys.contains(&0xffe1))
            .context("Нет Shift")? as u8
            + self.first;
        self.connection.grab_server()?.check()?;
        let result = (|| -> Result<Vec<(u8, bool)>> {
            anyhow::ensure!(
                self.input_focus()? == focus,
                "Фокус изменился; замена отменена"
            );
            self.idle()?;
            let mut requests = vec![];
            let mut injected = vec![];
            let mut key = |code, pressed, delay| -> Result<()> {
                injected.push((code, pressed));
                requests.push(self.connection.xtest_fake_input(
                    if pressed {
                        xproto::KEY_PRESS_EVENT
                    } else {
                        xproto::KEY_RELEASE_EVENT
                    },
                    code,
                    delay,
                    self.root,
                    0,
                    0,
                    0,
                )?);
                Ok(())
            };
            for _ in 0..count {
                key(backspace, true, 0)?;
                key(backspace, false, 0)?;
            }
            let mut current = self.layout()?;
            let mut plan = plan.into_iter().peekable();
            while let Some((group, code, shifted)) = plan.next() {
                if current != Some(group) {
                    self.switch(group)?;
                    current = Some(group);
                }
                if shifted {
                    key(shift, true, 0)?;
                }
                key(code, true, 0)?;
                // Let XTest process the last character before a different XKB group takes effect.
                let next_group = plan.peek().map(|next| next.0).unwrap_or(layout);
                key(code, false, u32::from(next_group != group))?;
                if shifted {
                    key(shift, false, 0)?;
                }
            }
            for request in requests {
                request.check()?;
            }
            self.switch(layout)?;
            Ok(injected)
        })();
        self.connection.ungrab_server()?.check()?;
        result
    }
    pub fn layout(&self) -> Result<Option<Layout>> {
        let state = self
            .connection
            .xkb_get_state(xkb::ID::USE_CORE_KBD.into())?
            .reply()?;
        Ok(self
            .groups
            .get(usize::from(u8::from(state.group)))
            .and_then(|name| match name.as_str() {
                "us" | "gb" => Some(Layout::En),
                "ru" => Some(Layout::Ru),
                _ => None,
            }))
    }
    pub fn switch(&self, layout: Layout) -> Result<()> {
        let group = self
            .groups
            .iter()
            .position(|name| match layout {
                Layout::En => matches!(name.as_str(), "us" | "gb"),
                Layout::Ru => name == "ru",
            })
            .context(
                "Нужная раскладка не настроена в системе; добавьте RU/EN в настройках клавиатуры",
            )?;
        self.connection
            .xkb_latch_lock_state(
                xkb::ID::USE_CORE_KBD.into(),
                0u16.into(),
                0u16.into(),
                true,
                (group as u8).into(),
                0u16.into(),
                false,
                0,
            )?
            .check()?;
        self.connection.flush()?;
        Ok(())
    }
    /// Plan against the existing XKB groups; never rewrite the user's keymap.
    pub fn typing_plan(&self, text: &str, preferred: Layout) -> Result<Vec<(Layout, u8, bool)>> {
        anyhow::ensure!(
            text.chars().count() <= 4096,
            "Слишком большой фрагмент для клавиатурной замены"
        );
        anyhow::ensure!(
            !text.chars().any(char::is_control),
            "Клавиатурная замена не поддерживает управляющие символы"
        );
        text.chars()
            .map(|ch| {
                for layout in [preferred, preferred.other()] {
                    let Some(group) = self.groups.iter().position(|name| match layout {
                        Layout::En => matches!(name.as_str(), "us" | "gb"),
                        Layout::Ru => name == "ru",
                    }) else {
                        continue;
                    };
                    for (index, key) in self.symbols_map.iter().enumerate() {
                        let group = if group >= usize::from(key.group_info & 15) {
                            0
                        } else {
                            group
                        };
                        for shift in 0..=1 {
                            if key
                                .syms
                                .get(group * usize::from(key.width) + shift)
                                .is_some_and(|symbol| {
                                    xkeysym::Keysym::new(*symbol).key_char() == Some(ch)
                                })
                            {
                                return Ok((layout, self.symbols_first + index as u8, shift != 0));
                            }
                        }
                    }
                }
                anyhow::bail!("Символ {ch:?} отсутствует в настроенных раскладках; текст сохранён")
            })
            .collect()
    }
    pub fn input_focus(&self) -> Result<u32> {
        Ok(self.connection.get_input_focus()?.reply()?.focus)
    }
    pub fn keys_released(&self) -> Result<bool> {
        Ok(self
            .connection
            .query_keymap()?
            .reply()?
            .keys
            .iter()
            .all(|byte| *byte == 0))
    }
    pub fn idle(&self) -> Result<()> {
        self.idle_except(None)
    }
    fn idle_except(&self, allowed: Option<u8>) -> Result<()> {
        let state = self
            .connection
            .xkb_get_state(xkb::ID::USE_CORE_KBD.into())?
            .reply()?;
        anyhow::ensure!(
            u16::from(state.mods) & 79 == 0,
            "Отпустите модификаторы и выключите Caps Lock перед преобразованием"
        );
        let keys = self.connection.query_keymap()?.reply()?;
        anyhow::ensure!(
            keys.keys.iter().enumerate().all(|(index, byte)| {
                let ignored = allowed
                    .filter(|key| usize::from(*key) / 8 == index)
                    .map_or(0, |key| 1 << (key % 8));
                byte & !ignored == 0
            }),
            "Дождитесь отпускания клавиш"
        );
        Ok(())
    }
    pub fn type_key(&self, code: u8, shift: bool) -> Result<()> {
        let shift_code = (0..self.keys.len() / self.width)
            .find(|i| self.symbols(self.first + *i as u8).contains(&0xffe1))
            .map(|i| self.first + i as u8)
            .context("Нет клавиши Shift")?;
        if shift {
            self.fake_key(shift_code, true)?;
        }
        let result = self
            .fake_key(code, true)
            .and_then(|()| self.fake_key(code, false));
        if shift {
            self.fake_key(shift_code, false)?;
        }
        self.connection.flush()?;
        result
    }
    fn fake_key(&self, code: u8, pressed: bool) -> Result<()> {
        self.connection
            .xtest_fake_input(
                if pressed {
                    xproto::KEY_PRESS_EVENT
                } else {
                    xproto::KEY_RELEASE_EVENT
                },
                code,
                x11rb::CURRENT_TIME,
                self.root,
                0,
                0,
                0,
            )?
            .check()?;
        Ok(())
    }
    pub fn backspace(&self) -> Result<()> {
        let code = (0..self.keys.len() / self.width)
            .find(|i| self.symbols(self.first + *i as u8).contains(&0xff08))
            .map(|i| self.first + i as u8)
            .context("Нет Backspace")?;
        self.type_key(code, false)
    }
    pub fn bind(&mut self, config: &Config) -> Result<Vec<String>> {
        let bindings: Vec<_> = config
            .hotkeys
            .iter()
            .filter(|binding| !binding.keys.is_empty())
            .map(|b| (b.action, b.keys.clone()))
            .collect();
        if self.bindings_valid && self.bindings == bindings {
            return Ok(self.failures.clone());
        }
        for (key, modifiers, _) in self.grabs.drain(..) {
            self.connection
                .ungrab_key(key, self.root, modifiers.into())?
                .check()?;
        }
        let modifier_map = self.connection.get_modifier_mapping()?.reply()?;
        let mut ignored = 2u16;
        for (modifier, keys) in modifier_map
            .keycodes
            .chunks(usize::from(modifier_map.keycodes_per_modifier()).max(1))
            .enumerate()
        {
            if keys.iter().any(|key| self.symbols(*key).contains(&0xff7f)) {
                ignored |= 1 << modifier;
            }
        }
        let mut failures = vec![];
        for (action, text) in &bindings {
            let hotkey = Hotkey::parse(text)?;
            let code = (0..self.keys.len() / self.width)
                .find(|index| {
                    self.keys[index * self.width..(index + 1) * self.width].contains(&hotkey.key)
                })
                .map(|index| self.first + index as u8);
            let Some(code) = code else {
                failures.push(text.clone());
                continue;
            };
            for locks in (0..=255u16).filter(|mask| mask & !ignored == 0) {
                let modifiers = hotkey.modifiers as u16 | locks;
                match self
                    .connection
                    .grab_key(
                        false,
                        self.root,
                        modifiers.into(),
                        code,
                        xproto::GrabMode::ASYNC,
                        xproto::GrabMode::ASYNC,
                    )?
                    .check()
                {
                    Ok(()) => self.grabs.push((code, modifiers, *action)),
                    Err(_) => failures.push(text.clone()),
                }
            }
        }
        if !self.enter_grabs.is_empty() && self.grab_enter().is_err() {
            failures.push("Enter".into());
        }
        self.bindings = bindings;
        self.bindings_valid = true;
        self.connection.flush()?;
        failures.sort();
        failures.dedup();
        self.failures = failures.clone();
        Ok(failures)
    }
    pub fn rebind(&mut self) {
        self.bindings_valid = false;
    }
    fn symbols(&self, code: u8) -> &[u32] {
        let start = usize::from(code.saturating_sub(self.first)) * self.width;
        self.keys.get(start..start + self.width).unwrap_or(&[])
    }
    pub fn grab_enter(&mut self) -> Result<()> {
        if self.enter_grabs.is_empty() {
            let map = self.connection.get_modifier_mapping()?.reply()?;
            let mut locks = 2u16;
            for (index, keys) in map
                .keycodes
                .chunks(usize::from(map.keycodes_per_modifier()).max(1))
                .enumerate()
            {
                if keys
                    .iter()
                    .any(|code| self.symbols(*code).contains(&0xff7f))
                {
                    locks |= 1 << index;
                }
            }
            for code in self.first..=self.connection.setup().max_keycode {
                if self
                    .symbols(code)
                    .iter()
                    .any(|symbol| matches!(*symbol, 0xff0d | 0xff8d))
                {
                    for modifiers in (0..=255u16).filter(|mask| mask & !locks == 0) {
                        self.enter_grabs.push((code, modifiers));
                    }
                }
            }
        }
        for &(code, modifiers) in &self.enter_grabs {
            if self
                .grabs
                .iter()
                .any(|(key, mods, _)| *key == code && *mods == modifiers)
            {
                continue;
            }
            self.connection
                .grab_key(
                    false,
                    self.root,
                    modifiers.into(),
                    code,
                    xproto::GrabMode::ASYNC,
                    xproto::GrabMode::SYNC,
                )?
                .check()?;
        }
        Ok(())
    }
    pub fn take_enter(&mut self) -> Option<xproto::KeyPressEvent> {
        self.enter.take()
    }
    /// Always release a captured Enter, including when correction was rejected.
    pub fn finish_enter(&mut self, event: xproto::KeyPressEvent) -> Result<()> {
        self.connection
            .allow_events(xproto::Allow::REPLAY_KEYBOARD, event.time)?
            .check()?;
        Ok(())
    }
    /// Keep real input frozen until edits have been queued ahead of the original Enter.
    pub fn replace_before_enter(
        &self,
        focus: u32,
        count: usize,
        text: &str,
        layout: Layout,
        event: &xproto::KeyPressEvent,
    ) -> Result<()> {
        let plan = self.typing_plan(text, layout)?;
        let backspace = (self.first..=self.connection.setup().max_keycode)
            .find(|code| self.symbols(*code).contains(&0xff08))
            .context("Нет Backspace")?;
        self.idle_except(Some(event.detail))?;
        anyhow::ensure!(self.input_focus()? == focus, "Фокус изменился");
        let send = |code: u8, group: Layout, shifted: bool| -> Result<()> {
            let group = self
                .groups
                .iter()
                .position(|name| match group {
                    Layout::En => matches!(name.as_str(), "us" | "gb"),
                    Layout::Ru => name == "ru",
                })
                .context("Неизвестная раскладка")?;
            for response_type in [xproto::KEY_PRESS_EVENT, xproto::KEY_RELEASE_EVENT] {
                let key = xproto::KeyPressEvent {
                    response_type,
                    detail: code,
                    event: focus,
                    root: self.root,
                    time: event.time,
                    same_screen: true,
                    state: ((group as u16) << 13 | u16::from(shifted)).into(),
                    ..Default::default()
                };
                self.connection
                    .send_event(
                        true,
                        focus,
                        if response_type == xproto::KEY_PRESS_EVENT {
                            xproto::EventMask::KEY_PRESS
                        } else {
                            xproto::EventMask::KEY_RELEASE
                        },
                        key,
                    )?
                    .check()?;
            }
            Ok(())
        };
        for _ in 0..count {
            send(backspace, layout, false)?;
        }
        for (group, code, shifted) in plan {
            send(code, group, shifted)?;
        }
        self.switch(layout)?;
        Ok(())
    }
    fn suppress_alt_menu(&self, event: &xproto::KeyPressEvent) -> Result<()> {
        if u16::from(event.state) & 0xcd != 8 {
            return Ok(());
        }
        let control = (self.first..=self.connection.setup().max_keycode)
            .find(|code| self.symbols(*code).contains(&0xffe4))
            .context("Нет Control_R")?;
        let focus = self.input_focus()?;
        // The grabbed main key is invisible to the app; prevent its Alt release becoming a menu tap.
        for response_type in [xproto::KEY_PRESS_EVENT, xproto::KEY_RELEASE_EVENT] {
            let key = xproto::KeyPressEvent {
                response_type,
                detail: control,
                event: focus,
                root: self.root,
                time: event.time,
                same_screen: true,
                state: (u16::from(event.state)
                    | if response_type == xproto::KEY_RELEASE_EVENT {
                        4
                    } else {
                        0
                    })
                .into(),
                ..Default::default()
            };
            self.connection
                .send_event(
                    true,
                    focus,
                    if response_type == xproto::KEY_PRESS_EVENT {
                        xproto::EventMask::KEY_PRESS
                    } else {
                        xproto::EventMask::KEY_RELEASE
                    },
                    key,
                )?
                .check()?;
        }
        self.connection.flush()?;
        Ok(())
    }
    pub fn actions(&mut self) -> Result<Vec<Action>> {
        let mut actions = vec![];
        while let Some(event) = self.connection.poll_for_event()? {
            match event {
                Event::KeyPress(key) => {
                    let modifiers = u16::from(key.state) & 255;
                    if let Some((_, _, action)) = self
                        .grabs
                        .iter()
                        .find(|(code, mods, _)| *code == key.detail && *mods == modifiers)
                    {
                        self.suppress_alt_menu(&key)?;
                        actions.push(*action);
                    } else if self.enter_grabs.contains(&(key.detail, modifiers)) {
                        self.enter = Some(key);
                    }
                }
                Event::MappingNotify(_) => {
                    self.mapping()?;
                    self.rebind();
                }
                _ => {}
            }
        }
        Ok(actions)
    }
}
