//! Состояние ввода IBus и общие действия: preedit, язык, исключения приложений и безопасный откат.

use crate::{
    config::{Action, Config},
    dictionary::Dictionaries,
    hotkey,
    layout::{self, Layout},
};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Commit(String),
    Preedit(String),
    Delete { offset: i32, count: u32 },
    Message(String),
    Bell,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surrounding {
    pub text: String,
    pub cursor: usize,
    pub anchor: usize,
}

impl Surrounding {
    pub fn valid(&self) -> bool {
        self.text.len() <= 1_048_576
            && self.cursor <= self.text.chars().count()
            && self.anchor <= self.text.chars().count()
    }
    fn replace(&self, start: usize, end: usize, text: &str) -> Self {
        let value = self
            .text
            .chars()
            .take(start)
            .chain(text.chars())
            .chain(self.text.chars().skip(end))
            .collect();
        let cursor = start + text.chars().count();
        Self {
            text: value,
            cursor,
            anchor: cursor,
        }
    }
}

#[derive(Clone, Debug)]
struct Undo {
    original: String,
    replacement: String,
    expected: Surrounding,
    previous_layout: Layout,
}

pub struct Engine {
    pub layout: Layout,
    pub buffer: String,
    pub app: String,
    pub title: String,
    pub purpose: u32,
    pub hints: u32,
    pub capabilities: u32,
    pub surrounding: Option<Surrounding>,
    pub excluded: bool,
    pub focus: String,
    undo: Option<Undo>,
    preedit_undo: Option<(String, Layout)>,
    remembered: HashMap<String, Layout>,
    paused: Vec<String>,
}

impl Engine {
    pub fn new(config: &Config) -> Self {
        Self {
            layout: config.initial_layout,
            buffer: String::new(),
            app: String::new(),
            title: String::new(),
            purpose: 0,
            hints: 0,
            capabilities: 0,
            surrounding: None,
            excluded: false,
            focus: String::new(),
            undo: None,
            preedit_undo: None,
            remembered: HashMap::new(),
            paused: Vec::new(),
        }
    }
    pub fn sensitive(&self) -> bool {
        self.purpose == 8 || self.purpose == 9 || self.hints & (1 << 11) != 0
    }
    pub fn automatic(&self, config: &Config) -> bool {
        config.automatic
            && !self.excluded
            && !matches!(self.purpose, 2..=6 | 8..=10)
            && !self.sensitive()
            && !self.paused.contains(&self.app)
            && self.capabilities & 1 != 0
    }
    pub fn focus_in(&mut self, focus: &str, app: &str, title: &str, config: &Config) {
        if self.focus != focus || self.app != app {
            self.buffer.clear();
            self.undo = None;
            self.preedit_undo = None;
            self.surrounding = None;
            if config.remember_layout {
                if !self.app.is_empty() {
                    self.remembered.insert(self.app.clone(), self.layout);
                }
                if let Some(layout) = self.remembered.get(app) {
                    self.layout = *layout;
                }
            }
        }
        self.focus = focus.into();
        // IBus omits an unchanged initial empty surrounding-text update.
        if self.surrounding.is_none() {
            self.surrounding = Some(Surrounding {
                text: String::new(),
                cursor: 0,
                anchor: 0,
            });
        }
        self.app = app.into();
        self.title = title.into();
        self.excluded = config
            .exceptions
            .iter()
            .any(|rule| rule.disable_auto && rule.matches(app, title));
        if let Some(layout) = config
            .exceptions
            .iter()
            .rev()
            .find(|rule| rule.layout.is_some() && rule.matches(app, title))
            .and_then(|r| r.layout)
        {
            self.layout = layout;
        }
    }
    pub fn set_surrounding(&mut self, value: Surrounding) {
        self.surrounding = if !self.sensitive() && value.valid() {
            Some(value)
        } else {
            None
        };
    }
    pub fn clear(&mut self) -> Vec<Effect> {
        self.buffer.clear();
        self.undo = None;
        self.preedit_undo = None;
        self.surrounding = None;
        vec![Effect::Preedit(String::new())]
    }
    pub fn flush(&mut self) -> Vec<Effect> {
        if self.buffer.is_empty() {
            return vec![];
        }
        let text = std::mem::take(&mut self.buffer);
        self.undo = None;
        self.preedit_undo = None;
        vec![Effect::Preedit(String::new()), Effect::Commit(text)]
    }
    pub fn process(
        &mut self,
        key: u32,
        state: u32,
        config: &mut Config,
        dictionaries: &Dictionaries,
    ) -> (bool, Vec<Effect>) {
        if state & hotkey::RELEASE != 0 {
            return (false, vec![]);
        }
        if self.sensitive() {
            return (false, self.clear());
        }
        if let Some(action) = config.action(key, state) {
            return (true, self.action(action, config));
        }
        if is_modifier(key) {
            return (false, vec![]);
        }
        self.preedit_undo = None;
        if state & (hotkey::CONTROL | hotkey::ALT | hotkey::SUPER | (1 << 26) | (1 << 27)) != 0 {
            let effects = self.flush();
            self.undo = None;
            return (false, effects);
        }
        if key == 0xff08 {
            if self.buffer.pop().is_some() {
                return (true, vec![Effect::Preedit(self.buffer.clone())]);
            }
            self.undo = None;
            return (false, vec![]);
        }
        if key == 0xff1b && !self.buffer.is_empty() {
            return (true, self.flush());
        }
        if matches!(key, 0xff0d | 0xff8d | 0xff09) {
            // Commit text, then let the application handle Enter/Tab (submit/focus).
            return (false, self.finish("", config, dictionaries));
        }
        let Some(ch) = key_char(key) else {
            let effects = self.flush();
            self.undo = None;
            return (false, effects);
        };
        let ch = self.layout.map(ch);
        if ch.is_control() {
            return (false, self.flush());
        }
        if !self.automatic(config) {
            let mut effects = self.flush();
            self.undo = None;
            effects.push(Effect::Commit(ch.to_string()));
            return (true, effects);
        }
        if layout::delimiter(ch) {
            return (true, self.finish(&ch.to_string(), config, dictionaries));
        }
        self.undo = None;
        self.buffer.push(ch);
        if self.buffer.chars().count() >= 128 {
            return (true, self.flush());
        }
        (true, vec![Effect::Preedit(self.buffer.clone())])
    }
    fn finish(
        &mut self,
        delimiter: &str,
        config: &Config,
        dictionaries: &Dictionaries,
    ) -> Vec<Effect> {
        self.preedit_undo = None;
        if self.buffer.is_empty() {
            return if delimiter.is_empty() {
                vec![]
            } else {
                self.undo = None;
                vec![Effect::Commit(delimiter.into())]
            };
        }
        let original = std::mem::take(&mut self.buffer);
        let previous_layout = self.layout;
        let mut output = original.clone();
        if self.automatic(config)
            && let Some(correction) = dictionaries.replacement(&original, self.layout, config)
        {
            output = correction.text;
            self.layout = correction.layout;
        }
        let changed = output != original;
        let original = original + delimiter;
        let output = output + delimiter;
        self.undo = if changed {
            self.surrounding.as_ref().filter(|s| s.valid()).map(|s| {
                let start = s.cursor.min(s.anchor);
                let end = s.cursor.max(s.anchor);
                Undo {
                    original,
                    replacement: output.clone(),
                    expected: s.replace(start, end, &output),
                    previous_layout,
                }
            })
        } else {
            None
        };
        let mut effects = vec![Effect::Preedit(String::new()), Effect::Commit(output)];
        if changed && config.sound {
            effects.push(Effect::Bell);
        }
        effects
    }
    pub fn action(&mut self, action: Action, config: &mut Config) -> Vec<Effect> {
        if self.sensitive() {
            return vec![Effect::Message(
                "Действие отключено в защищённом поле".into(),
            )];
        }
        match action {
            Action::ToggleAuto => {
                let effects = self.flush();
                config.automatic = !config.automatic;
                effects
            }
            Action::SwitchLayout => {
                let effects = self.flush();
                self.layout = self.layout.other();
                effects
            }
            Action::PauseApp => {
                let effects = self.flush();
                if let Some(index) = self.paused.iter().position(|a| a == &self.app) {
                    self.paused.remove(index);
                } else {
                    self.paused.push(self.app.clone());
                }
                effects
            }
            Action::Undo => self.undo(),
            Action::ConvertWord if !self.buffer.is_empty() => {
                self.preedit_undo = Some((self.buffer.clone(), self.layout));
                self.buffer = layout::convert_from(&self.buffer, self.layout);
                self.layout = self.layout.other();
                vec![Effect::Preedit(self.buffer.clone())]
            }
            Action::ConvertSelection
            | Action::ChangeCase
            | Action::Transliterate
            | Action::ConvertWord => self.transform(action),
        }
    }
    fn transform(&mut self, action: Action) -> Vec<Effect> {
        if !self.buffer.is_empty() {
            return vec![Effect::Message("Сначала завершите ввод слова".into())];
        }
        let Some(s) = self
            .surrounding
            .clone()
            .filter(|s| s.valid() && self.capabilities & 32 != 0)
        else {
            return vec![Effect::Message(
                "Приложение не предоставляет surrounding text; преобразование недоступно".into(),
            )];
        };
        let (start, end) = if action == Action::ConvertWord {
            if s.cursor != s.anchor {
                (s.cursor.min(s.anchor), s.cursor.max(s.anchor))
            } else {
                let chars: Vec<_> = s.text.chars().collect();
                let mut end = s.cursor;
                while end > 0 && chars[end - 1].is_whitespace() {
                    end -= 1;
                }
                let mut start = end;
                while start > 0 && !chars[start - 1].is_whitespace() {
                    start -= 1;
                }
                (start, end)
            }
        } else {
            (s.cursor.min(s.anchor), s.cursor.max(s.anchor))
        };
        if start == end {
            return vec![Effect::Message("Выделите текст в приложении".into())];
        }
        let original: String = s.text.chars().skip(start).take(end - start).collect();
        let replacement = match action {
            Action::ChangeCase => {
                if original
                    .chars()
                    .filter(|ch| ch.is_alphabetic())
                    .all(|ch| ch.is_uppercase())
                {
                    original.to_lowercase()
                } else {
                    original.to_uppercase()
                }
            }
            Action::Transliterate => layout::transliterate(&original),
            _ => layout::convert_selection(&original),
        };
        if original == replacement {
            return vec![];
        }
        // A selected range is replaced by CommitText itself; deleting it first duplicates deletion in GTK/Qt.
        let mut effects = if s.cursor == s.anchor {
            vec![Effect::Delete {
                offset: start as i32 - s.cursor as i32,
                count: (end - start) as u32,
            }]
        } else {
            vec![]
        };
        // Last-word conversion can leave trailing spaces: insert at the deletion point, not after them.
        if s.cursor == s.anchor && end != s.cursor {
            let tail: String = s.text.chars().skip(end).take(s.cursor - end).collect();
            effects.clear();
            effects.push(Effect::Delete {
                offset: start as i32 - s.cursor as i32,
                count: (s.cursor - start) as u32,
            });
            let full = replacement.clone() + &tail;
            effects.push(Effect::Commit(full.clone()));
            self.undo = Some(Undo {
                original: original + &tail,
                replacement: full.clone(),
                expected: s.replace(start, s.cursor, &full),
                previous_layout: self.layout,
            });
        } else {
            effects.push(Effect::Commit(replacement.clone()));
            self.undo = Some(Undo {
                original,
                replacement: replacement.clone(),
                expected: s.replace(start, end, &replacement),
                previous_layout: self.layout,
            });
        }
        if matches!(action, Action::ConvertSelection | Action::ConvertWord) {
            self.layout = if replacement.chars().any(layout::is_russian) {
                Layout::Ru
            } else {
                Layout::En
            };
        }
        effects
    }
    fn undo(&mut self) -> Vec<Effect> {
        if let Some((original, layout)) = self.preedit_undo.take() {
            self.buffer = original;
            self.layout = layout;
            return vec![Effect::Preedit(self.buffer.clone())];
        }
        if !self.buffer.is_empty() {
            return vec![Effect::Message(
                "Откат недоступен после нового ввода".into(),
            )];
        }
        let Some(undo) = self.undo.take() else {
            return vec![Effect::Message("Нет преобразования для отмены".into())];
        };
        if self.capabilities & 32 == 0 || self.surrounding.as_ref() != Some(&undo.expected) {
            return vec![Effect::Message(
                "Текст или позиция курсора изменились; откат отменён".into(),
            )];
        }
        self.layout = undo.previous_layout;
        let count = undo.replacement.chars().count() as u32;
        vec![
            Effect::Delete {
                offset: -(count as i32),
                count,
            },
            Effect::Commit(undo.original),
        ]
    }
}

fn is_modifier(key: u32) -> bool {
    (0xffe1..=0xffee).contains(&key) || key == 0xfe03
}
fn key_char(key: u32) -> Option<char> {
    if (32..=126).contains(&key) {
        char::from_u32(key)
    } else if key & 0xff000000 == 0x01000000 {
        char::from_u32(key & 0x00ffffff)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (Engine, Config, Dictionaries) {
        let config = Config::default();
        let mut engine = Engine::new(&config);
        engine.capabilities = 33;
        engine.set_surrounding(Surrounding {
            text: "".into(),
            cursor: 0,
            anchor: 0,
        });
        (engine, config, Dictionaries::fixture())
    }
    fn type_text(e: &mut Engine, c: &mut Config, d: &Dictionaries, text: &str) -> Vec<Effect> {
        text.chars()
            .flat_map(|ch| e.process(ch as u32, 0, c, d).1)
            .collect()
    }
    #[test]
    fn enter_commits_correction_before_forwarding_the_key() {
        for key in [0xff0d, 0xff8d] {
            let (mut engine, mut config, dictionaries) = setup();
            type_text(&mut engine, &mut config, &dictionaries, "ghbdtn");
            let (handled, effects) = engine.process(key, 0, &mut config, &dictionaries);
            assert!(!handled);
            assert!(effects.contains(&Effect::Commit("привет".into())));
            assert_eq!(engine.layout, Layout::Ru);
        }
    }
    #[test]
    fn domains_are_not_corrected_at_the_first_dot() {
        let (mut e, mut c, d) = setup();
        assert!(
            type_text(&mut e, &mut c, &d, "ghbdtn.com ")
                .contains(&Effect::Commit("ghbdtn.com ".into()))
        );
        assert!(
            type_text(&mut e, &mut c, &d, "ghbdtn. ").contains(&Effect::Commit("привет. ".into()))
        );
    }
    #[test]
    fn punctuation_keys_can_belong_to_russian_words() {
        for (original, expected) in [
            ("ghjdthz. ", "проверяю "),
            ("k.,k. ", "люблю "),
            (";bpym ", "жизнь "),
        ] {
            let (mut e, mut c, d) = setup();
            let effects = type_text(&mut e, &mut c, &d, original);
            assert!(
                effects.contains(&Effect::Commit(expected.into())),
                "{original}: {effects:?}"
            );
        }
    }
    #[test]
    fn preedit_conversion_can_be_undone_without_surrounding() {
        let (mut e, mut c, d) = setup();
        e.surrounding = None;
        type_text(&mut e, &mut c, &d, "ghbdtn");
        assert_eq!(
            e.action(Action::ConvertWord, &mut c),
            vec![Effect::Preedit("привет".into())]
        );
        assert_eq!(
            e.action(Action::Undo, &mut c),
            vec![Effect::Preedit("ghbdtn".into())]
        );
        assert_eq!(e.layout, Layout::En);
        e.action(Action::ConvertWord, &mut c);
        type_text(&mut e, &mut c, &d, "a");
        assert!(matches!(
            e.action(Action::Undo, &mut c).as_slice(),
            [Effect::Message(_)]
        ));
    }
    #[test]
    fn correct_then_verified_undo() {
        let (mut e, mut c, d) = setup();
        let effects = type_text(&mut e, &mut c, &d, "ghbdtn ");
        assert!(effects.contains(&Effect::Commit("привет ".into())));
        assert_eq!(e.layout, Layout::Ru);
        e.set_surrounding(Surrounding {
            text: "привет ".into(),
            cursor: 7,
            anchor: 7,
        });
        assert_eq!(
            e.action(Action::Undo, &mut c),
            vec![
                Effect::Delete {
                    offset: -7,
                    count: 7
                },
                Effect::Commit("ghbdtn ".into())
            ]
        );
        assert_eq!(e.layout, Layout::En);
    }
    #[test]
    fn undo_does_not_delete_unrelated_text() {
        let (mut e, mut c, d) = setup();
        type_text(&mut e, &mut c, &d, "ghbdtn ");
        e.set_surrounding(Surrounding {
            text: "other".into(),
            cursor: 5,
            anchor: 5,
        });
        assert!(matches!(
            e.action(Action::Undo, &mut c).as_slice(),
            [Effect::Message(_)]
        ));
    }
    #[test]
    fn selected_unicode_is_replaced_without_double_deletion() {
        let (mut e, mut c, _) = setup();
        e.set_surrounding(Surrounding {
            text: "😀 ghbdtn! ok".into(),
            cursor: 9,
            anchor: 2,
        });
        assert_eq!(
            e.action(Action::ConvertSelection, &mut c),
            vec![Effect::Commit("привет!".into())]
        );
        e.set_surrounding(Surrounding {
            text: "😀 привет! ok".into(),
            cursor: 9,
            anchor: 9,
        });
        assert!(
            e.action(Action::Undo, &mut c)
                .contains(&Effect::Commit("ghbdtn!".into()))
        );
    }
    #[test]
    fn shortcuts_passwords_exceptions_and_new_input() {
        let (mut e, mut c, d) = setup();
        type_text(&mut e, &mut c, &d, "ghb");
        assert!(!e.process('c' as u32, hotkey::CONTROL, &mut c, &d).0);
        assert!(e.buffer.is_empty());
        e.purpose = 8;
        assert!(!e.process('a' as u32, 0, &mut c, &d).0);
        assert!(e.buffer.is_empty());
        e.purpose = 0;
        e.focus_in("a", "org.gnome.Terminal", "", &c);
        assert!(!e.automatic(&c));
        e.focus_in("b", "gedit", "", &c);
        assert!(e.automatic(&c));
        type_text(&mut e, &mut c, &d, "ghbdtn ");
        type_text(&mut e, &mut c, &d, "a");
        assert!(matches!(
            e.action(Action::Undo, &mut c).as_slice(),
            [Effect::Message(_)]
        ));
    }
    #[test]
    fn snippets_and_manual_last_word() {
        let (mut e, mut c, d) = setup();
        c.replacements
            .insert("sig".into(), "С уважением,\nИван".into());
        assert!(
            type_text(&mut e, &mut c, &d, "sig ")
                .contains(&Effect::Commit("С уважением,\nИван ".into()))
        );
        e.set_surrounding(Surrounding {
            text: "ghbdtn  ".into(),
            cursor: 8,
            anchor: 8,
        });
        assert_eq!(
            e.action(Action::ConvertWord, &mut c),
            vec![
                Effect::Delete {
                    offset: -8,
                    count: 8
                },
                Effect::Commit("привет  ".into())
            ]
        );
    }
    #[test]
    fn pause_forces_an_unknown_word_without_dictionary_or_smart() {
        let (mut e, mut c, d) = setup();
        c.smart = false;
        assert!(d.correct("qzxqzx", Layout::En, &c).is_none());
        type_text(&mut e, &mut c, &d, "qzxqzx");
        assert_eq!(
            e.process(0xff13, 0, &mut c, &d),
            (true, vec![Effect::Preedit("йячйяч".into())])
        );
        assert_eq!(e.layout, Layout::Ru);
        assert_eq!(
            e.action(Action::Undo, &mut c),
            vec![Effect::Preedit("qzxqzx".into())]
        );
        c.automatic = false;
        e.buffer.clear();
        e.set_surrounding(Surrounding {
            text: "qzxqzx".into(),
            cursor: 6,
            anchor: 6,
        });
        assert!(
            e.action(Action::ConvertWord, &mut c)
                .contains(&Effect::Commit("йячйяч".into()))
        );
    }
    #[test]
    fn no_surrounding_or_invalid_positions_are_safe() {
        let (mut e, mut c, _) = setup();
        e.set_surrounding(Surrounding {
            text: "text".into(),
            cursor: 99,
            anchor: 0,
        });
        assert!(matches!(
            e.action(Action::ConvertSelection, &mut c).as_slice(),
            [Effect::Message(_)]
        ));
        e.layout = Layout::Ru;
        assert_eq!(
            e.process(
                'a' as u32,
                hotkey::RELEASE,
                &mut c,
                &Dictionaries::fixture()
            ),
            (false, vec![])
        );
    }
}
