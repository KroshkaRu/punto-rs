//! Проверка и атомарное сохранение конфигурации; старые файлы используют значения полей по умолчанию.

use crate::{hotkey::Hotkey, layout::Layout};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ToggleAuto,
    SwitchLayout,
    ConvertSelection,
    ConvertWord,
    Undo,
    ChangeCase,
    Transliterate,
    PauseApp,
}

impl Action {
    pub fn id(self) -> &'static str {
        match self {
            Self::ToggleAuto => "toggle_auto",
            Self::SwitchLayout => "switch_layout",
            Self::ConvertSelection => "convert_selection",
            Self::ConvertWord => "convert_word",
            Self::Undo => "undo",
            Self::ChangeCase => "change_case",
            Self::Transliterate => "transliterate",
            Self::PauseApp => "pause_app",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        [
            Self::ToggleAuto,
            Self::SwitchLayout,
            Self::ConvertSelection,
            Self::ConvertWord,
            Self::Undo,
            Self::ChangeCase,
            Self::Transliterate,
            Self::PauseApp,
        ]
        .into_iter()
        .find(|action| action.id() == id)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::ToggleAuto => "Автопереключение вкл./выкл.",
            Self::SwitchLayout => "Переключить RU / EN",
            Self::ConvertSelection => "Исправить выделенный текст",
            Self::ConvertWord => "Исправить последнее слово",
            Self::Undo => "Отменить преобразование",
            Self::ChangeCase => "Изменить регистр выделения",
            Self::Transliterate => "Транслитерация выделения",
            Self::PauseApp => "Пауза для приложения",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub action: Action,
    pub keys: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppRule {
    pub app: String,
    pub title: String,
    pub disable_auto: bool,
    pub layout: Option<Layout>,
}

impl AppRule {
    pub fn matches(&self, app: &str, title: &str) -> bool {
        (!self.app.is_empty() || !self.title.is_empty())
            && (self.app.is_empty() || app.to_lowercase().contains(&self.app.to_lowercase()))
            && (self.title.is_empty() || title.to_lowercase().contains(&self.title.to_lowercase()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub backend: Backend,
    pub automatic: bool,
    pub initial_layout: Layout,
    pub remember_layout: bool,
    pub minimum_word: usize,
    pub skip_uppercase: bool,
    pub sound: bool,
    pub english_dictionary: PathBuf,
    pub russian_dictionary: PathBuf,
    pub english_words: Vec<String>,
    pub russian_words: Vec<String>,
    pub ignored_words: Vec<String>,
    pub rules: BTreeMap<String, String>,
    pub replacements: BTreeMap<String, String>,
    pub exceptions: Vec<AppRule>,
    pub hotkeys: Vec<Binding>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: Backend::Auto,
            automatic: true,
            initial_layout: Layout::En,
            remember_layout: true,
            minimum_word: 3,
            skip_uppercase: true,
            sound: false,
            english_dictionary: "/usr/share/hunspell/en_US".into(),
            russian_dictionary: "/usr/share/hunspell/ru_RU".into(),
            english_words: vec![],
            russian_words: vec![],
            ignored_words: vec![],
            rules: BTreeMap::new(),
            replacements: BTreeMap::new(),
            exceptions: [
                "terminal",
                "konsole",
                "alacritty",
                "kitty",
                "wezterm",
                "keepass",
                "1password",
                "punto-rs",
            ]
            .iter()
            .map(|s| AppRule {
                app: s.to_string(),
                disable_auto: true,
                ..Default::default()
            })
            .collect(),
            hotkeys: [
                (Action::ToggleAuto, "Ctrl+Alt+A"),
                (Action::SwitchLayout, "Ctrl+Alt+Space"),
                (Action::ConvertSelection, "Ctrl+Alt+S"),
                (Action::ConvertWord, "Pause"),
                (Action::Undo, "ScrollLock"),
                (Action::ChangeCase, "Ctrl+Alt+U"),
                (Action::Transliterate, "Ctrl+Alt+T"),
                (Action::PauseApp, "Ctrl+Alt+P"),
            ]
            .into_iter()
            .map(|(action, keys)| Binding {
                action,
                keys: keys.into(),
            })
            .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Auto,
    Native,
    Ibus,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let config: Self = toml::from_str(&text).context("Повреждён файл настроек")?;
                config.validate()?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).context("Не удалось прочитать настройки"),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !(2..=32).contains(&self.minimum_word) {
            bail!("Минимальная длина слова: 2–32");
        }
        let mut seen = Vec::new();
        for binding in &self.hotkeys {
            let key = Hotkey::parse(&binding.keys)?;
            if seen.contains(&key) {
                bail!("Повтор горячей клавиши: {}", binding.keys);
            }
            seen.push(key);
        }
        for (from, to) in self.rules.iter().chain(&self.replacements) {
            if from.is_empty()
                || from.contains(char::is_whitespace)
                || to.is_empty()
                || to.len() > 65536
                || to.contains('\0')
            {
                bail!("Правило должно иметь непустые ключ без пробелов и значение до 64 КиБ");
            }
        }
        Ok(())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path.parent().context("Нет каталога настроек")?;
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(path)
            .context("Не удалось сохранить настройки")?;
        Ok(())
    }
    pub fn action(&self, key: u32, state: u32) -> Option<Action> {
        self.hotkeys.iter().find_map(|binding| {
            Hotkey::parse(&binding.keys)
                .ok()
                .filter(|hotkey| hotkey.matches(key, state))
                .map(|_| binding.action)
        })
    }
}

pub fn config_path() -> PathBuf {
    config_home().join("punto-rs/config.toml")
}

pub(crate) fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_roundtrip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::load(&path).unwrap();
        config.save(&path).unwrap();
        assert!(Config::load(&path).unwrap().automatic);
        config.hotkeys[1].keys = config.hotkeys[0].keys.clone();
        assert!(config.save(&path).is_err());
        assert!(Config::load(&path).is_ok());
        fs::write(path.clone(), "automatic = 12").unwrap();
        assert!(Config::load(&path).is_err());
    }
    #[test]
    fn exception_requires_both_conditions() {
        let rule = AppRule {
            app: "firefox".into(),
            title: "code".into(),
            ..Default::default()
        };
        assert!(rule.matches("Firefox", "Code review"));
        assert!(!rule.matches("Firefox", "Mail"));
        assert!(!AppRule::default().matches("firefox", "Code"));
    }
}
