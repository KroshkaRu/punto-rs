//! Соответствие клавиш QWERTY/ЙЦУКЕН. Индексы редактирования считаются в символах, а не байтах UTF-8.

use serde::{Deserialize, Serialize};

const EN: &str = "`qwertyuiop[]asdfghjkl;'zxcvbnm,./~QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>?@#$^&|";
const RU: &str = "ёйцукенгшщзхъфывапролджэячсмитьбю.ЁЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,\"№;:?/";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    En,
    Ru,
}

impl Layout {
    pub fn other(self) -> Self {
        match self {
            Self::En => Self::Ru,
            Self::Ru => Self::En,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::En => "EN",
            Self::Ru => "RU",
        }
    }
    pub fn map(self, ch: char) -> char {
        match self {
            Self::En => ch,
            Self::Ru => EN
                .chars()
                .zip(RU.chars())
                .find_map(|(a, b)| (a == ch).then_some(b))
                .unwrap_or(ch),
        }
    }
}

pub fn is_russian(ch: char) -> bool {
    ('а'..='я').contains(&ch.to_ascii_lowercase())
        || ('А'..='Я').contains(&ch)
        || ch == 'ё'
        || ch == 'Ё'
}

pub fn delimiter(ch: char) -> bool {
    // Commas, dots, colons and semicolons can be letters in the other layout.
    ch.is_whitespace() || "!?".contains(ch)
}

pub fn word_layout(word: &str, fallback: Layout) -> Layout {
    if word.chars().any(is_russian) {
        Layout::Ru
    } else if word.chars().any(|ch| ch.is_ascii_alphabetic()) {
        Layout::En
    } else {
        fallback
    }
}
pub fn word_range(text: &str, cursor: usize) -> Option<(usize, usize)> {
    let chars: Vec<_> = text.chars().collect();
    if cursor > chars.len() {
        return None;
    }
    let mut end = cursor;
    while end > 0 && delimiter(chars[end - 1]) {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && !delimiter(chars[start - 1]) {
        start -= 1;
    }
    (start < end).then_some((start, end))
}
pub fn completed_word(text: &str, cursor: usize) -> Option<(usize, String, String)> {
    let (start, end) = word_range(text, cursor)?;
    if end + 1 != cursor {
        return None;
    }
    Some((
        start,
        text.chars().skip(start).take(end - start).collect(),
        text.chars().skip(end).take(1).collect(),
    ))
}

pub fn convert(text: &str) -> String {
    text.chars()
        .map(|ch| {
            let pairs = EN.chars().zip(RU.chars());
            if is_russian(ch) {
                pairs
                    .into_iter()
                    .find_map(|(en, ru)| (ru == ch).then_some(en))
                    .unwrap_or(ch)
            } else {
                pairs
                    .into_iter()
                    .find_map(|(en, ru)| (en == ch).then_some(ru))
                    .unwrap_or(ch)
            }
        })
        .collect()
}

pub fn convert_from(text: &str, source: Layout) -> String {
    let pairs: Vec<_> = EN.chars().zip(RU.chars()).collect();
    text.chars()
        .map(|ch| {
            pairs
                .iter()
                .find_map(|&(en, ru)| match source {
                    Layout::En => (en == ch).then_some(ru),
                    Layout::Ru => (ru == ch).then_some(en),
                })
                .unwrap_or(ch)
        })
        .collect()
}

/// Convert prose by the alphabet of each word; punctuation follows that word.
pub fn convert_selection(text: &str) -> String {
    let mut result = String::new();
    for token in text.split_inclusive(char::is_whitespace) {
        let source = if token.chars().any(is_russian) {
            Layout::Ru
        } else {
            Layout::En
        };
        result.push_str(&convert_from(token, source));
    }
    result
}

pub fn transliterate(text: &str) -> String {
    const LETTERS: &str = "абвгдеёжзийклмнопрстуфхцчшщъыьэюя";
    const LATIN: [&str; 33] = [
        "a", "b", "v", "g", "d", "e", "yo", "zh", "z", "i", "y", "k", "l", "m", "n", "o", "p", "r",
        "s", "t", "u", "f", "kh", "ts", "ch", "sh", "shch", "", "y", "", "e", "yu", "ya",
    ];
    let mut result = String::new();
    for ch in text.chars() {
        let lower = ch.to_lowercase().next().unwrap_or(ch);
        if let Some(index) = LETTERS.chars().position(|v| v == lower) {
            let value = LATIN[index];
            if ch.is_uppercase() {
                let mut chars = value.chars();
                if let Some(first) = chars.next() {
                    result.extend(first.to_uppercase());
                    result.extend(chars);
                }
            } else {
                result.push_str(value);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_tables_match() {
        assert_eq!(EN.chars().count(), RU.chars().count());
    }
    #[test]
    fn words_and_case() {
        assert_eq!(convert("Ghbdtn"), "Привет");
        assert_eq!(convert("руддщ"), "hello");
        assert_eq!(convert_selection("Ghbdtn vbh!"), "Привет мир!");
        assert_eq!(convert_selection("Привет мир!"), "Ghbdtn vbh!");
        assert_eq!(convert_from(&convert_from(EN, Layout::En), Layout::Ru), EN);
    }
    #[test]
    fn symbols_and_unicode() {
        assert_eq!(convert_selection("ghbdtn\nруддщ 😀"), "привет\nhello 😀");
        assert_eq!(Layout::Ru.map('@'), '"');
        assert_eq!(transliterate("Привет, Ёж!"), "Privet, Yozh!");
    }
}
