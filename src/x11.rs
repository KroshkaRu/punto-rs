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
    pub fn bind(&mut self, config: &Config) -> Result<Vec<String>> {
        let bindings: Vec<_> = config
            .hotkeys
            .iter()
            .map(|b| (b.action, b.keys.clone()))
            .collect();
        if self.bindings == bindings {
            return Ok(vec![]);
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
        self.connection.flush()?;
        failures.sort();
        failures.dedup();
        Ok(failures)
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
                    self.bindings.clear();
                }
                _ => {}
            }
        }
        Ok(actions)
    }
}
