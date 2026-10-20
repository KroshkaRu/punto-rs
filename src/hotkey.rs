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
}

#[cfg(test)]
mod tests {
    use super::*;
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
