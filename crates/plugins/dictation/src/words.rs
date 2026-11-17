//! Custom words and the cleanup Handy applies to a transcript before it
//! pastes it, from Handy's `src-tauri/src/audio_toolkit/text.rs` (see
//! `docs/provenance/handy.md`). A word or run of up to three words that
//! sounds or reads like one of your custom words becomes that word:
//! Levenshtein distance for spelling, Soundex for sound, so "Charge B"
//! becomes "ChargeBee". Filler-word removal stays behind, as the spec asks.

use natural::phonetics::soundex;
use strsim::levenshtein;

/// Handy's default `word_correction_threshold`: the highest combined
/// score a match may have, from 0 for the same word.
pub(crate) const THRESHOLD: f64 = 0.18;

/// The longest custom word, in characters, as Handy allows.
pub(crate) const LONGEST_WORD: usize = 50;

/// A custom word as you typed it, the way Handy's settings clean it:
/// without `<`, `>`, `"` and `'`, one space between words and none at
/// either end. `None` when nothing is left or it's too long.
pub(crate) fn clean_word(word: &str) -> Option<String> {
    let kept: String = word
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | '"' | '\''))
        .collect();
    let cleaned = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    (!cleaned.is_empty() && cleaned.chars().count() <= LONGEST_WORD).then_some(cleaned)
}

/// The text to paste for a transcript: custom words applied, then
/// repeated words collapsed and spaces tidied. Empty when the transcript
/// holds no letter or digit, as for a clip of silence that came back as
/// ".".
pub(crate) fn finish(raw: &str, custom_words: &[String]) -> String {
    let corrected = apply_custom_words(raw, custom_words, THRESHOLD);
    let text = collapse_stutters(&corrected).trim().to_owned();
    if text.chars().any(char::is_alphanumeric) {
        text
    } else {
        String::new()
    }
}

/// Strips punctuation from each word, lowercases, and joins without
/// spaces, so "Charge B" matches "ChargeBee".
fn build_ngram(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| build_match_key(word))
        .collect::<Vec<_>>()
        .concat()
}

fn build_match_key(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

struct CustomWordMatchKey {
    word_index: usize,
    key: String,
}

fn build_custom_word_match_keys(word: &str, word_index: usize) -> Vec<CustomWordMatchKey> {
    let primary_key = build_match_key(word);
    let mut keys = Vec::with_capacity(2);

    // The matcher takes ASCII terms only: its whitespace tokenization and
    // Soundex suit no CJK script.
    if is_supported_fuzzy_key(&primary_key) {
        keys.push(CustomWordMatchKey {
            word_index,
            key: primary_key.clone(),
        });
    }

    if word.contains('&') {
        let expanded_key = build_match_key(&word.replace('&', " and "));
        if is_supported_fuzzy_key(&expanded_key) && expanded_key != primary_key {
            keys.push(CustomWordMatchKey {
                word_index,
                key: expanded_key,
            });
        }
    }

    keys
}

fn is_supported_fuzzy_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric())
}

fn supports_soundex(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphabetic())
}

/// The custom word closest to `candidate`, a cleaned and lowercased
/// string, with its score, when one scores under `threshold`.
fn find_best_match<'a>(
    candidate: &str,
    custom_words: &'a [String],
    custom_word_match_keys: &[CustomWordMatchKey],
    threshold: f64,
) -> Option<(&'a String, f64)> {
    if !is_supported_fuzzy_key(candidate) || candidate.chars().count() > 50 {
        return None;
    }

    let mut best_match: Option<&String> = None;
    let mut best_score = f64::MAX;

    for custom_word_key in custom_word_match_keys {
        // Skip lengths too far apart, at most 25% or 2 characters, so an
        // n-gram doesn't match a much shorter word.
        let candidate_len = candidate.chars().count();
        let custom_word_len = custom_word_key.key.chars().count();
        let len_diff = candidate_len.abs_diff(custom_word_len) as f64;
        let max_len = candidate_len.max(custom_word_len) as f64;
        let max_allowed_diff = (max_len * 0.25).max(2.0);
        if len_diff > max_allowed_diff {
            continue;
        }

        let levenshtein_dist = levenshtein(candidate, &custom_word_key.key);
        let levenshtein_score = if max_len > 0.0 {
            levenshtein_dist as f64 / max_len
        } else {
            1.0
        };

        // Soundex is English and ASCII: numbers get no phonetic boost.
        let phonetic_match = supports_soundex(candidate)
            && supports_soundex(&custom_word_key.key)
            && soundex(candidate, &custom_word_key.key);

        let combined_score = if phonetic_match {
            levenshtein_score * 0.3
        } else {
            levenshtein_score
        };

        if combined_score < threshold && combined_score < best_score {
            best_match = Some(&custom_words[custom_word_key.word_index]);
            best_score = combined_score;
        }
    }

    best_match.map(|word| (word, best_score))
}

/// `text` with each word, or run of up to three words, that matches one of
/// `custom_words` under `threshold` replaced by it, keeping the original's
/// punctuation and capitals.
pub(crate) fn apply_custom_words(text: &str, custom_words: &[String], threshold: f64) -> String {
    if custom_words.is_empty() {
        return text.to_owned();
    }

    let custom_word_match_keys: Vec<CustomWordMatchKey> = custom_words
        .iter()
        .enumerate()
        .flat_map(|(index, word)| build_custom_word_match_keys(word, index))
        .collect();

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut result = Vec::new();
    let mut i = 0;

    while i < words.len() {
        let mut best_match: Option<(usize, &String, f64)> = None;

        // N-grams up to three words, the closest match winning, so a
        // longest-first match doesn't swallow an ordinary word that shares
        // a Soundex code.
        for n in (1..=3).rev() {
            if i + n > words.len() {
                continue;
            }

            let ngram_words = &words[i..i + n];
            // No match crosses punctuation: in "Charge B, che" the comma
            // closes the candidate at "B,".
            if ngram_words[..n.saturating_sub(1)]
                .iter()
                .any(|word| !extract_punctuation(word).1.is_empty())
            {
                continue;
            }
            let ngram = build_ngram(ngram_words);

            if let Some((replacement, score)) =
                find_best_match(&ngram, custom_words, &custom_word_match_keys, threshold)
            {
                let is_better = best_match
                    .as_ref()
                    .is_none_or(|(_, _, best_score)| score < *best_score);
                if is_better {
                    best_match = Some((n, replacement, score));
                }
            }
        }

        if let Some((n, replacement, _)) = best_match {
            let ngram_words = &words[i..i + n];
            let (prefix, _) = extract_punctuation(ngram_words[0]);
            let (_, suffix) = extract_punctuation(ngram_words[n - 1]);
            let corrected = preserve_case_pattern(ngram_words[0], replacement);
            result.push(format!("{prefix}{corrected}{suffix}"));
            i += n;
        } else {
            result.push(words[i].to_owned());
            i += 1;
        }
    }

    result.join(" ")
}

/// `replacement` in the case pattern of `original`: all capitals, a
/// capital first letter, or as written.
fn preserve_case_pattern(original: &str, replacement: &str) -> String {
    if original.chars().all(char::is_uppercase) {
        replacement.to_uppercase()
    } else if original.chars().next().is_some_and(char::is_uppercase) {
        let mut chars: Vec<char> = replacement.chars().collect();
        if let Some(first_char) = chars.get_mut(0) {
            *first_char = first_char.to_uppercase().next().unwrap_or(*first_char);
        }
        chars.into_iter().collect()
    } else {
        replacement.to_owned()
    }
}

/// The punctuation before a word's first letter or digit and after its
/// last one.
fn extract_punctuation(word: &str) -> (&str, &str) {
    // Byte offsets from char_indices, so multibyte punctuation such as `。`
    // never splits.
    let prefix_end = word
        .char_indices()
        .find(|(_, c)| c.is_alphanumeric())
        .map_or(word.len(), |(index, _)| index);
    let suffix_start = word
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map_or(0, |(index, c)| index + c.len_utf8());

    let prefix = if prefix_end > 0 {
        &word[..prefix_end]
    } else {
        ""
    };
    let suffix = if suffix_start < word.len() {
        &word[suffix_start..]
    } else {
        ""
    };
    (prefix, suffix)
}

/// A word said three or more times in a row, once: "I I I I" is "I".
fn collapse_stutters(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return text.to_owned();
    }

    let mut result: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let word_lower = word.to_lowercase();
        let mut count = 1;
        if word_lower.chars().all(char::is_alphabetic) {
            while i + count < words.len() && words[i + count].to_lowercase() == word_lower {
                count += 1;
            }
        }
        result.push(word);
        i += if count >= 3 { count } else { 1 };
    }
    result.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        apply_custom_words, clean_word, collapse_stutters, extract_punctuation, finish,
        preserve_case_pattern,
    };

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn an_exact_match_takes_the_custom_spelling() {
        assert_eq!(
            apply_custom_words("hello world", &words(&["Hello", "World"]), 0.5),
            "Hello World"
        );
    }

    #[test]
    fn a_misspelling_close_enough_becomes_the_custom_word() {
        assert_eq!(
            apply_custom_words("helo wrold", &words(&["hello", "world"]), 0.5),
            "hello world"
        );
    }

    #[test]
    fn case_follows_the_spoken_word() {
        assert_eq!(preserve_case_pattern("HELLO", "world"), "WORLD");
        assert_eq!(preserve_case_pattern("Hello", "world"), "World");
        assert_eq!(preserve_case_pattern("hello", "WORLD"), "WORLD");
    }

    #[test]
    fn punctuation_stays_around_the_word() {
        assert_eq!(extract_punctuation("hello"), ("", ""));
        assert_eq!(extract_punctuation("!hello?"), ("!", "?"));
        assert_eq!(extract_punctuation("...hello..."), ("...", "..."));
        assert_eq!(extract_punctuation("你好。"), ("", "。"));
        assert_eq!(extract_punctuation("「你好」"), ("「", "」"));
    }

    #[test]
    fn no_custom_words_change_nothing() {
        assert_eq!(apply_custom_words("hello world", &[], 0.5), "hello world");
    }

    #[test]
    fn two_spoken_words_become_one_custom_word() {
        let result = apply_custom_words(
            "il cui nome è Charge B, che permette",
            &words(&["ChargeBee"]),
            0.5,
        );
        assert!(result.contains("ChargeBee,"), "{result}");
        assert!(!result.contains("Charge B"));
    }

    #[test]
    fn three_spoken_words_become_one_custom_word() {
        let result = apply_custom_words("use Chat G P T for this", &words(&["ChatGPT"]), 0.5);
        assert!(result.contains("ChatGPT"), "{result}");
    }

    #[test]
    fn the_closest_ngram_wins() {
        assert_eq!(
            apply_custom_words("Open AI GPT model", &words(&["OpenAI", "GPT"]), 0.5),
            "OpenAI GPT model"
        );
        assert_eq!(
            apply_custom_words("using Mac Book Pro", &words(&["MacBook Pro"]), 0.5),
            "using MacBook Pro"
        );
    }

    #[test]
    fn an_ngram_keeps_all_capitals() {
        let result = apply_custom_words("CHARGE B is great", &words(&["ChargeBee"]), 0.5);
        assert!(result.contains("CHARGEBEE"), "{result}");
    }

    #[test]
    fn a_trailing_number_isnt_counted_twice() {
        let result = apply_custom_words("use GPT4 for this", &words(&["GPT-4"]), 0.5);
        assert!(!result.contains("GPT-44"), "{result}");
    }

    #[test]
    fn an_ampersand_word_matches_written_and_spoken() {
        for text in [
            "send it to RD for review",
            "send it to R and D for review",
            "send it to R&D for review",
        ] {
            assert_eq!(
                apply_custom_words(text, &words(&["R&D"]), 0.18),
                "send it to R&D for review"
            );
        }
    }

    #[test]
    fn unicode_punctuation_survives_and_cjk_isnt_matched() {
        assert_eq!(
            apply_custom_words("「Handee。」", &words(&["Handy"]), 0.5),
            "「Handy。」"
        );
        assert_eq!(
            apply_custom_words("你好。", &words(&["你号"]), 1.0),
            "你好。"
        );
    }

    #[test]
    fn the_default_threshold_fixes_a_name_and_leaves_ordinary_words() {
        assert_eq!(
            finish("ask noora about it", &words(&["Noura"])),
            "ask Noura about it"
        );
        assert_eq!(finish("the cat sat", &words(&["Noura"])), "the cat sat");
    }

    #[test]
    fn a_stutter_collapses_and_spaces_tidy() {
        assert_eq!(collapse_stutters("I I I I think"), "I think");
        assert_eq!(collapse_stutters("no no way"), "no no way");
        assert_eq!(finish("  so   I I I said  ", &[]), "so I said");
    }

    #[test]
    fn a_transcript_without_words_pastes_nothing() {
        assert_eq!(finish("", &[]), "");
        assert_eq!(finish(" . ", &[]), "");
        assert_eq!(finish("?", &words(&["Noura"])), "");
    }

    #[test]
    fn a_custom_word_is_cleaned_as_handy_cleans_it() {
        assert_eq!(clean_word("  Mac   Book "), Some("Mac Book".to_owned()));
        assert_eq!(clean_word("<\"O'Brien\">"), Some("OBrien".to_owned()));
        assert_eq!(clean_word("   "), None);
        assert_eq!(clean_word(&"a".repeat(51)), None);
        assert_eq!(clean_word(&"a".repeat(50)), Some("a".repeat(50)));
    }
}
