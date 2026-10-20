//! Общие словари Hunspell, пользовательские правила и различение пунктуации и букв другой раскладки.

use crate::{
    config::Config,
    layout::{Layout, convert_from},
};
use anyhow::{Context, Result};
use spellbook::Dictionary;
use std::{fs, path::Path};

pub struct Dictionaries {
    english: Dictionary,
    russian: Dictionary,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Correction {
    pub text: String,
    pub layout: Layout,
}

impl Dictionaries {
    pub fn load(config: &Config) -> Result<Self> {
        Ok(Self {
            english: load(&config.english_dictionary)?,
            russian: load(&config.russian_dictionary)?,
        })
    }
    pub fn check(&self, word: &str, layout: Layout, config: &Config) -> bool {
        let custom = match layout {
            Layout::En => &config.english_words,
            Layout::Ru => &config.russian_words,
        };
        custom
            .iter()
            .any(|v| v.to_lowercase() == word.to_lowercase())
            || match layout {
                Layout::En => self.english.check(word) || self.english.check(&word.to_lowercase()),
                Layout::Ru => self.russian.check(word) || self.russian.check(&word.to_lowercase()),
            }
    }
    pub fn correct(&self, word: &str, layout: Layout, config: &Config) -> Option<Correction> {
        let full = word;
        let plain = full.trim_matches(edge_punctuation);
        if full.chars().count() > 128
            || config.ignored_words.iter().any(|ignored| {
                ignored.to_lowercase() == full.to_lowercase()
                    || ignored.to_lowercase() == plain.to_lowercase()
            })
        {
            return None;
        }
        // Try the whole token before stripping punctuation: "." may be the Russian letter "ю".
        let mut start = 0;
        while start < full.len() {
            let mut end = full.len();
            while start < end {
                let word = &full[start..end];
                if let Some(mut correction) = self.correct_word(word, layout, config) {
                    correction.text = full[..start].to_owned() + &correction.text + &full[end..];
                    return Some(correction);
                }
                let last = word.chars().next_back()?;
                if !edge_punctuation(last) {
                    break;
                }
                end -= last.len_utf8();
            }
            let first = full[start..].chars().next()?;
            if !edge_punctuation(first) {
                break;
            }
            start += first.len_utf8();
        }
        None
    }
    fn correct_word(&self, word: &str, layout: Layout, config: &Config) -> Option<Correction> {
        if word.is_empty()
            || config
                .ignored_words
                .iter()
                .any(|v| v.eq_ignore_ascii_case(word) || v.to_lowercase() == word.to_lowercase())
        {
            return None;
        }
        if let Some(to) = config.rules.get(word) {
            return Some(Correction {
                text: to.clone(),
                layout: if to.chars().any(crate::layout::is_russian) {
                    Layout::Ru
                } else {
                    Layout::En
                },
            });
        }
        let length = word.chars().count();
        // The pronoun I is a word, so the abbreviation guard must not discard it.
        let pronoun_i = layout == Layout::Ru && matches!(word, "ш" | "Ш");
        if (length > 2 && length < config.minimum_word)
            || word
                .chars()
                .any(|ch| ch.is_numeric() || "@/\\_=".contains(ch))
            || (config.skip_uppercase
                && !pronoun_i
                && word
                    .chars()
                    .filter(|c| c.is_alphabetic())
                    .all(|c| c.is_uppercase()))
            || self.check(word, layout, config)
        {
            return None;
        }
        let text = if pronoun_i {
            "I".into()
        } else {
            convert_from(word, layout)
        };
        if text == word || !self.check(&text, layout.other(), config) {
            return None;
        }
        Some(Correction {
            text,
            layout: layout.other(),
        })
    }
    pub fn replacement(&self, word: &str, layout: Layout, config: &Config) -> Option<Correction> {
        if let Some(text) = config.replacements.get(word) {
            Some(Correction {
                text: text.clone(),
                layout: crate::layout::word_layout(text, layout),
            })
        } else {
            self.correct(word, layout, config)
        }
    }
}

fn edge_punctuation(ch: char) -> bool {
    ".,;:'\"()[]{}<>«»…—–".contains(ch)
}

fn load(base: &Path) -> Result<Dictionary> {
    let aff = fs::read_to_string(base.with_extension("aff")).with_context(|| {
        format!(
            "Нет словаря {}.aff; установите hunspell-en-us hunspell-ru",
            base.display()
        )
    })?;
    let dic = fs::read_to_string(base.with_extension("dic"))
        .with_context(|| format!("Нет словаря {}.dic", base.display()))?;
    Dictionary::new(&aff, &dic).map_err(|e| anyhow::anyhow!("Словарь {}: {e}", base.display()))
}

#[cfg(test)]
impl Dictionaries {
    pub fn fixture() -> Self {
        Self {
            english: Dictionary::new("SET UTF-8", "12\nhello\nworld\ntest\ngood\nwe\nus\nto\nin\nit\nof\nvs\nI").unwrap(),
            russian: Dictionary::new(
                "SET UTF-8",
                "19\nпривет\nмир\nтест\nдобро\nработа\nпроверяю\nлюблю\nжизнь\nхочу\nда\nне\nон\nиз\nна\nпо\nже\nмы\nя\nи",
            )
            .unwrap(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn english_pronoun_i_is_not_an_abbreviation() {
        let dictionaries = Dictionaries::fixture();
        let mut config = Config::default();
        for (word, expected) in [("Ш", "I"), ("ш", "I"), ("«Ш».", "«I».")] {
            let correction = dictionaries.correct(word, Layout::Ru, &config).unwrap();
            assert_eq!(correction.text, expected);
            assert_eq!(correction.layout, Layout::En);
        }
        assert!(dictionaries.correct("I", Layout::En, &config).is_none());
        assert!(
            dictionaries
                .correct("GHBDTN", Layout::En, &config)
                .is_none()
        );
        config.ignored_words.push("Ш".into());
        assert!(dictionaries.correct("Ш", Layout::Ru, &config).is_none());
        config.ignored_words.clear();
        config.rules.insert("Ш".into(), "другое".into());
        assert_eq!(
            dictionaries.correct("Ш", Layout::Ru, &config).unwrap().text,
            "другое"
        );
    }
    #[test]
    fn short_dictionary_words_work_in_both_directions() {
        let dictionaries = Dictionaries::fixture();
        let mut config = Config::default();
        for (word, source, expected) in [
            ("цу", Layout::Ru, "we"),
            ("гы", Layout::Ru, "us"),
            ("ещ", Layout::Ru, "to"),
            ("шт", Layout::Ru, "in"),
            ("ше", Layout::Ru, "it"),
            ("ща", Layout::Ru, "of"),
            ("lf", Layout::En, "да"),
            ("yt", Layout::En, "не"),
            ("jy", Layout::En, "он"),
            ("bp", Layout::En, "из"),
            (";t", Layout::En, "же"),
            ("z", Layout::En, "я"),
        ] {
            let correction = dictionaries.correct(word, source, &config).unwrap();
            assert_eq!(correction.text, expected, "{word}");
            assert_eq!(correction.layout, source.other());
        }
        assert!(dictionaries.correct("vs", Layout::En, &config).is_none());
        assert!(dictionaries.correct("мы", Layout::Ru, &config).is_none());
        config.ignored_words.push("цу".into());
        assert!(dictionaries.correct("цу", Layout::Ru, &config).is_none());
        config.minimum_word = 6;
        assert!(dictionaries.correct("руддщ", Layout::Ru, &config).is_none());
        assert!(dictionaries.correct("lf", Layout::En, &config).is_some());
        config.replacements.insert("sig".into(), "Привет".into());
        assert_eq!(
            dictionaries
                .replacement("sig", Layout::En, &config)
                .unwrap()
                .layout,
            Layout::Ru
        );
    }
    #[test]
    fn punctuation_can_be_letters_or_surround_the_word() {
        let dictionaries = Dictionaries::fixture();
        let config = Config::default();
        for (original, expected) in [
            ("ghjdthz.", "проверяю"),
            ("k.,k.", "люблю"),
            (";bpym", "жизнь"),
            ("[jxe", "хочу"),
            ("ghbdtn.", "привет."),
            ("ghjdthz.,", "проверяю,"),
            ("(ghjdthz.),", "(проверяю),"),
            ("«ghbdtn»", "«привет»"),
            ("\"ghbdtn\"", "\"привет\""),
        ] {
            assert_eq!(
                dictionaries
                    .correct(original, Layout::En, &config)
                    .unwrap()
                    .text,
                expected,
                "{original}"
            );
        }
        assert_eq!(
            dictionaries.correct("«руддщ».中", Layout::Ru, &config),
            None
        );
        assert_eq!(
            dictionaries
                .correct("«руддщ».", Layout::Ru, &config)
                .unwrap()
                .text,
            "«hello»."
        );
        for original in [
            "hello.",
            "\"hello\"",
            "ghbdtn.com",
            "user@example.com",
            "https://ghbdtn.com",
            "3.14",
        ] {
            assert!(
                dictionaries
                    .correct(original, Layout::En, &config)
                    .is_none(),
                "{original}"
            );
        }
    }
    #[test]
    fn conservative_detection() {
        let dictionaries = Dictionaries::fixture();
        let mut config = Config::default();
        assert_eq!(
            dictionaries
                .correct("ghbdtn", Layout::En, &config)
                .unwrap()
                .text,
            "привет"
        );
        assert_eq!(
            dictionaries
                .correct("руддщ", Layout::Ru, &config)
                .unwrap()
                .text,
            "hello"
        );
        for word in [
            "hello",
            "xyzzy",
            "GHBDTN",
            "user@example.com",
            "ghbdtn2",
            "ab",
        ] {
            assert!(
                dictionaries.correct(word, Layout::En, &config).is_none(),
                "{word}"
            );
        }
        config.ignored_words.push("ghbdtn".into());
        assert!(
            dictionaries
                .correct("ghbdtn", Layout::En, &config)
                .is_none()
        );
        config.english_words.push("ghbdtn".into());
        config.ignored_words.clear();
        assert!(
            dictionaries
                .correct("ghbdtn", Layout::En, &config)
                .is_none()
        );
        config.rules.insert("abc".into(), "привет".into());
        assert_eq!(
            dictionaries
                .correct("abc", Layout::En, &config)
                .unwrap()
                .layout,
            Layout::Ru
        );
    }
}
