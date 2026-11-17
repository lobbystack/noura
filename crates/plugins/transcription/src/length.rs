//! How long a Summary may be, from how long its Transcript is: anarlog's
//! length policy (`crates/summary`) and the guidance line its desktop app
//! adds to the prompt (`formatSummaryLengthGuidance`), for a summary
//! without template sections. See `docs/provenance/anarlog.md`.

const MIN_SUMMARY_CHARACTERS: usize = 320;
const SECTION_GUIDANCE_CHARACTER_STEP: usize = 2_000;
const MAX_GUIDANCE_SECTIONS: usize = 8;

/// The guidance for one Transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    pub(crate) transcript_characters: usize,
    pub(crate) max_characters: usize,
    pub(crate) min_sections: usize,
    pub(crate) max_sections: usize,
}

/// The policy for a Transcript of `transcript_characters` characters, or
/// `None` for an empty one.
pub(crate) fn policy(transcript_characters: usize) -> Option<Policy> {
    if transcript_characters == 0 {
        return None;
    }
    let characters = transcript_characters as f64;
    let min_sections = (characters / (SECTION_GUIDANCE_CHARACTER_STEP * 2) as f64)
        .ceil()
        .clamp(1.0, 5.0);
    let max_sections = (1.0 + (characters / SECTION_GUIDANCE_CHARACTER_STEP as f64).ceil())
        .clamp(2.0, MAX_GUIDANCE_SECTIONS as f64);
    Some(Policy {
        transcript_characters,
        max_characters: transcript_characters.max(MIN_SUMMARY_CHARACTERS),
        // Both are whole numbers from 1 to 8.
        min_sections: min_sections as usize,
        max_sections: max_sections as usize,
    })
}

/// The policy for the non-empty `texts`, joined by spaces.
pub(crate) fn policy_for_texts(texts: &[&str]) -> Option<Policy> {
    let joined = texts
        .iter()
        .filter(|text| !text.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    policy(count_normalized_characters(&joined))
}

/// The line the prompt ends with, or `None` without a policy. A custom
/// prompt keeps its own structure and only gets the character limit.
pub(crate) fn guidance(policy: Option<&Policy>, custom_format: bool) -> Option<String> {
    let policy = policy?;
    let sections = if policy.min_sections == policy.max_sections {
        let plural = if policy.max_sections == 1 { "" } else { "s" };
        format!("exactly {} section{plural}", policy.max_sections)
    } else {
        format!(
            "{} to {} sections",
            policy.min_sections, policy.max_sections
        )
    };
    let limit = if custom_format {
        format!(
            "Keep the requested structure and stay under {} characters overall.",
            policy.max_characters
        )
    } else {
        format!(
            "Keep the summary proportional to it: use {sections} and stay under {} characters overall.",
            policy.max_characters
        )
    };
    Some(
        [
            format!(
                "Summary length: the transcript contains about {} characters.",
                policy.transcript_characters
            ),
            limit,
            "A short meeting must produce a short summary; never pad with filler.".to_owned(),
        ]
        .join(" "),
    )
}

/// The prompt with the guidance after it, as anarlog's
/// `withLengthGuidance` adds it.
pub(crate) fn with_guidance(prompt: &str, policy: Option<&Policy>, custom_format: bool) -> String {
    match guidance(policy, custom_format) {
        Some(guidance) => format!("{prompt}\n\n{guidance}"),
        None => prompt.to_owned(),
    }
}

/// Characters after collapsing each run of whitespace into one space and
/// trimming, counted as code points, as JavaScript counts them.
pub(crate) fn count_normalized_characters(text: &str) -> usize {
    let mut collapsed = String::with_capacity(text.len());
    let mut in_whitespace = false;
    for character in text.chars() {
        if is_js_whitespace(character) {
            if !in_whitespace {
                collapsed.push(' ');
            }
            in_whitespace = true;
        } else {
            collapsed.push(character);
            in_whitespace = false;
        }
    }
    collapsed.trim_matches(is_js_whitespace).chars().count()
}

fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_code_points_after_collapsing_javascript_whitespace() {
        assert_eq!(count_normalized_characters("이번 회의는 짧음"), 9);
        assert_eq!(count_normalized_characters("😀  meeting\tnotes"), 15);
        assert_eq!(
            count_normalized_characters("\u{FEFF}A\u{0085} B\u{FEFF}"),
            4
        );
    }

    #[test]
    fn the_policy_matches_anarlogs_cases() {
        let cases = [
            (200, 320, 1, 2),
            (636, 636, 1, 2),
            (6_000, 6_000, 2, 4),
            (10_000, 10_000, 3, 6),
            (30_000, 30_000, 5, 8),
        ];
        for (characters, max, min_sections, max_sections) in cases {
            assert_eq!(
                policy(characters),
                Some(Policy {
                    transcript_characters: characters,
                    max_characters: max,
                    min_sections,
                    max_sections,
                })
            );
        }
        assert_eq!(policy(0), None);
        assert_eq!(
            policy_for_texts(&["first", "", "second"]).map(|p| p.transcript_characters),
            Some(12)
        );
    }

    #[test]
    fn the_guidance_names_the_sections_and_the_limit() {
        let short = policy(200);
        assert_eq!(
            guidance(short.as_ref(), false).unwrap(),
            "Summary length: the transcript contains about 200 characters. Keep the summary \
             proportional to it: use 1 to 2 sections and stay under 320 characters overall. A \
             short meeting must produce a short summary; never pad with filler."
        );
        assert!(
            guidance(short.as_ref(), true)
                .unwrap()
                .contains("Keep the requested structure and stay under 320 characters")
        );
        assert_eq!(with_guidance("Prompt", None, false), "Prompt");
    }
}
