//! Разбор сочетаний и сопоставление модификаторов без регистрации системных переключателей языка.

use anyhow::{Result, bail};

pub const SHIFT: u32 = 1;
pub const CONTROL: u32 = 1 << 2;
pub const ALT: u32 = 1 << 3;
pub const SUPER: u32 = 1 << 6;
pub const RELEASE: u32 = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub key: u32,
    pub modifiers: u32,
}

impl Hotkey {
    pub fn parse(text: &str) -> Result<Self> {
        let mut modifiers = 0;
        let mut key = None;
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers |= CONTROL,
                "alt" => modifiers |= ALT,
                "shift" => modifiers |= SHIFT,
                "super" | "win" => modifiers |= SUPER,
                name => {
                    if key.is_some() {
                        bail!("Нужна одна клавиша: {text}");
                    }
                    key = Some(match name {
                        "space" => 32,
                        "backspace" => 0xff08,
                        "tab" => 0xff09,
                        "enter" | "return" => 0xff0d,
                        "escape" | "esc" => 0xff1b,
                        "pause" => 0xff13,
                        "scrolllock" => 0xff14,
                        "delete" => 0xffff,
                        "insert" => 0xff63,
                        "home" => 0xff50,
                        "end" => 0xff57,
                        "left" => 0xff51,
                        "up" => 0xff52,
                        "right" => 0xff53,
                        "down" => 0xff54,
                        "pageup" => 0xff55,
                        "pagedown" => 0xff56,
                        "print" => 0xff61,
                        "menu" => 0xff67,
                        _ if name.starts_with('f')
                            && name[1..]
                                .parse::<u32>()
                                .is_ok_and(|n| (1..=24).contains(&n)) =>
                        {
                            0xffbd + name[1..].parse::<u32>()?
                        }
                        _ if name.len() == 1 && name.as_bytes()[0].is_ascii_alphanumeric() => {
                            name.as_bytes()[0] as u32
                        }
                        _ => bail!("Неизвестная клавиша: {part}"),
                    });
                }
            }
        }
        Ok(Self {
            key: key.ok_or_else(|| anyhow::anyhow!("Не указана клавиша: {text}"))?,
            modifiers,
        })
    }
    pub fn matches(self, key: u32, state: u32) -> bool {
        let key = if key < 128 {
            (key as u8).to_ascii_lowercase() as u32
        } else {
            key
        };
        key == self.key
            && state & (SHIFT | CONTROL | ALT | SUPER) == self.modifiers
            && state & RELEASE == 0
    }
    pub fn parts(self) -> [String; 3] {
        let mut modifiers: Vec<_> = [
            (CONTROL, "Ctrl"),
            (ALT, "Alt"),
            (SHIFT, "Shift"),
            (SUPER, "Super"),
        ]
        .into_iter()
        .filter_map(|(mask, name)| (self.modifiers & mask != 0).then_some(name))
        .collect();
        let second = if modifiers.len() > 1 {
            modifiers.pop().unwrap()
        } else {
            ""
        };
        let key = match self.key {
            32 => "Space".into(),
            0xff08 => "Backspace".into(),
            0xff09 => "Tab".into(),
            0xff0d => "Enter".into(),
            0xff1b => "Escape".into(),
            0xff13 => "Pause".into(),
            0xff14 => "ScrollLock".into(),
            0xffff => "Delete".into(),
            0xff63 => "Insert".into(),
            0xff50 => "Home".into(),
            0xff57 => "End".into(),
            0xff51 => "Left".into(),
            0xff52 => "Up".into(),
            0xff53 => "Right".into(),
            0xff54 => "Down".into(),
            0xff55 => "PageUp".into(),
            0xff56 => "PageDown".into(),
            0xff61 => "Print".into(),
            0xff67 => "Menu".into(),
            0xffbe..=0xffd5 => format!("F{}", self.key - 0xffbd),
            _ => char::from_u32(self.key)
                .unwrap()
                .to_ascii_uppercase()
                .to_string(),
        };
        [modifiers.join("+"), second.into(), key]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropdown_parts_roundtrip() {
        for mask in 0..16 {
            for key in [
                "A",
                "0",
                "Space",
                "Pause",
                "ScrollLock",
                "F24",
                "PageDown",
                "Menu",
            ] {
                let modifiers = ["Ctrl", "Alt", "Shift", "Super"]
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, s)| s)
                    .chain([key])
                    .collect::<Vec<_>>()
                    .join("+");
                let original = Hotkey::parse(&modifiers).unwrap();
                let text = original
                    .parts()
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("+");
                assert_eq!(Hotkey::parse(&text).unwrap(), original);
            }
        }
    }
    #[test]
    fn parse_and_match() {
        let key = Hotkey::parse("Ctrl+Alt+S").unwrap();
        assert!(key.matches('S' as u32, CONTROL | ALT | 2));
        assert!(!key.matches('s' as u32, CONTROL));
        assert!(!key.matches('s' as u32, CONTROL | ALT | RELEASE));
        assert_eq!(Hotkey::parse("Shift+F12").unwrap().key, 0xffc9);
        for bad in ["", "Ctrl", "A+B", "Ctrl++", "F25"] {
            assert!(Hotkey::parse(bad).is_err(), "{bad}");
        }
    }
}
