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
        Ok(())
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
    pub fn typing_plan(&self, text: &str) -> Result<Vec<(Layout, u8, bool)>> {
        anyhow::ensure!(
            text.chars().count() <= 4096,
            "Слишком большой фрагмент для клавиатурной замены"
        );
        anyhow::ensure!(
            !text.chars().any(char::is_control),
            "Клавиатурная замена не поддерживает управляющие символы"
        );
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
        let symbols = map.map.syms_rtrn.context("XKB не вернул символы клавиш")?;
        text.chars()
            .map(|ch| {
                for layout in [Layout::En, Layout::Ru] {
                    let Some(group) = self.groups.iter().position(|name| match layout {
                        Layout::En => matches!(name.as_str(), "us" | "gb"),
                        Layout::Ru => name == "ru",
                    }) else {
                        continue;
                    };
                    for (index, key) in symbols.iter().enumerate() {
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
                                return Ok((layout, map.first_key_sym + index as u8, shift != 0));
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
    pub fn idle(&self) -> Result<()> {
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
            keys.keys.iter().all(|byte| *byte == 0),
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
                        actions.push(*action);
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
