//! Готовые статистические модели Smart; пересчёт доступен только отдельным скриптом.

use crate::layout::is_russian;

include!("../assets/smart/trigrams.rs");

pub(crate) fn english_probability(en: &str, ru: &str) -> Option<f64> {
    // Internal punctuation may be a domain or code; dictionary matches still handle it.
    if !en.chars().all(|ch| ch.is_ascii_alphabetic())
        || !ru.chars().all(is_russian)
        || en.chars().count() <= 3
    {
        return None;
    }
    let (en_score, en_seen) = ENGLISH.score(en);
    let (ru_score, ru_seen) = RUSSIAN.score(ru);
    if !en_seen && !ru_seen {
        return None;
    }
    // Sum log probabilities instead of multiplying tiny probabilities; equal language priors.
    let delta = ru_score - en_score;
    Some(if delta >= 0.0 {
        let odds = (-delta).exp();
        odds / (1.0 + odds)
    } else {
        1.0 / (1.0 + delta.exp())
    })
}

struct Trigrams {
    weights: &'static [([char; 3], u32, f64)],
    missing: f64,
}

impl Trigrams {
    fn score(&self, word: &str) -> (f64, bool) {
        let letters: Vec<_> = word.to_lowercase().chars().collect();
        let mut score = 0.0;
        let mut seen = false;
        for window in letters.windows(3) {
            let key = [window[0], window[1], window[2]];
            if let Ok(index) = self.weights.binary_search_by_key(&key, |entry| entry.0) {
                score += self.weights[index].2;
                seen = true;
            } else {
                score += self.missing;
            }
        }
        (score, seen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Layout, convert_from};

    #[test]
    fn bundled_statistics_are_sorted_complete_and_normalized() {
        for model in [&ENGLISH, &RUSSIAN] {
            assert!(model.weights.windows(2).all(|pair| pair[0].0 < pair[1].0));
            let total: u64 = model.weights.iter().map(|row| u64::from(row.1)).sum();
            let support = 33usize.pow(3);
            assert!((model.missing.exp() - 1.0 / (total as f64 + support as f64)).abs() < 1e-15);
            let mass: f64 = model.weights.iter().map(|row| row.2.exp()).sum::<f64>()
                + (support - model.weights.len()) as f64 * model.missing.exp();
            assert!((mass - 1.0).abs() < 1e-12);
            for row in model.weights {
                assert!(
                    (row.2.exp() - (f64::from(row.1) + 1.0) / (total as f64 + support as f64))
                        .abs()
                        < 1e-15
                );
            }
        }
    }

    #[test]
    fn probabilities_are_finite_and_leave_unsupported_tokens_alone() {
        let encoded = convert_from("приветики", Layout::Ru);
        let p_ru = english_probability(&encoded, "приветики").unwrap();
        let p_en =
            english_probability("examplesx", &convert_from("examplesx", Layout::En)).unwrap();
        assert!(p_ru.is_finite() && (0.0..0.5).contains(&p_ru));
        assert!(p_en.is_finite() && p_en > 0.5 && p_en <= 1.0);
        for word in ["abc", "qzxqzx", "foo.com", "foo_bar", "foo2"] {
            assert_eq!(
                english_probability(word, &convert_from(word, Layout::En)),
                None,
                "{word}"
            );
        }
    }
}
