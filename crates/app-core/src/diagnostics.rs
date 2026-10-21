//! The local diagnostic log and the report behind Copy diagnostics.
//!
//! The report gives the app version, the system, the open workspace's
//! indexed file count, its problem counts by kind and the last lines of
//! the log. Before a log line goes in, every name Noura knows from the
//! workspace and the recent list is replaced, then paths, file names,
//! fields that carry names and quoted text.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use unicode_normalization::UnicodeNormalization;

use crate::VERSION;

/// The log file's name in the log folder. Rotated files add `.1` to `.3`.
pub const LOG_FILE: &str = "noura.log";

/// How many recent log lines a report includes.
const REPORT_LINES: usize = 200;

/// Names shorter than this stay in log lines unless they sit in a path,
/// a quote or a named field: replacing every short word would leave
/// nothing to read.
const SHORTEST_NAME: usize = 3;

/// Where the local diagnostic log lives: a `logs` folder in the app data
/// folder.
pub fn log_folder(app_data: &Path) -> PathBuf {
    app_data.join("logs")
}

/// What the report says about the open workspace.
pub(crate) struct WorkspaceHealth {
    pub(crate) indexed_files: u64,
    /// Each kind of problem, such as `parse_error`, and how many there are.
    pub(crate) problems: BTreeMap<String, usize>,
}

/// Build the report. `names` are the names to replace in log lines.
pub(crate) fn report(
    log_folder: &Path,
    workspace: Option<&WorkspaceHealth>,
    names: impl IntoIterator<Item = String>,
) -> String {
    let mut report = format!("Noura {VERSION}\n");
    report.push_str(&format!(
        "System: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    match workspace {
        Some(health) => {
            report.push_str("Workspace: open\n");
            report.push_str(&format!("Indexed files: {}\n", health.indexed_files));
            let problems = if health.problems.is_empty() {
                "none".to_owned()
            } else {
                health
                    .problems
                    .iter()
                    .map(|(kind, count)| format!("{kind} {count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            report.push_str(&format!("Problems: {problems}\n"));
        }
        None => report.push_str("Workspace: none open\n"),
    }
    report.push_str(&format!("\nRecent log (last {REPORT_LINES} lines):\n"));
    let lines = recent_lines(log_folder);
    if lines.is_empty() {
        report.push_str("(no log entries)\n");
    } else {
        let redactor = Redactor::new(names);
        for line in lines {
            report.push_str(&redactor.line(&line));
            report.push('\n');
        }
    }
    report
}

/// The last lines of the log: the current file's, after the newest
/// rotated file's when the current one is short.
fn recent_lines(folder: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    for file in [format!("{LOG_FILE}.1"), LOG_FILE.to_owned()] {
        match std::fs::read(folder.join(&file)) {
            Ok(bytes) => lines.extend(String::from_utf8_lossy(&bytes).lines().map(str::to_owned)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "a diagnostic log file could not be read"),
        }
    }
    let start = lines.len().saturating_sub(REPORT_LINES);
    lines.split_off(start)
}

/// Replaces anything in a log line that could name a person's files or
/// content.
struct Redactor {
    /// Finds the known names, ignoring ASCII case.
    names: Option<AhoCorasick>,
}

impl Redactor {
    fn new(names: impl IntoIterator<Item = String>) -> Self {
        let mut patterns: Vec<String> = Vec::new();
        for name in names {
            let name = name.trim();
            // A file's name without its extension shows up in logs too.
            let stem = name
                .rsplit_once('.')
                .map(|(stem, _)| stem.trim())
                .filter(|stem| !stem.is_empty());
            for name in std::iter::once(name).chain(stem) {
                if name.chars().count() < SHORTEST_NAME || !name.chars().any(char::is_alphanumeric)
                {
                    continue;
                }
                // macOS file names often arrive decomposed.
                patterns.push(name.nfc().collect());
                patterns.push(name.nfd().collect());
            }
        }
        patterns.sort();
        patterns.dedup();
        let names = if patterns.is_empty() {
            None
        } else {
            AhoCorasickBuilder::new()
                .ascii_case_insensitive(true)
                .match_kind(MatchKind::Standard)
                .build(&patterns)
                .inspect_err(|error| tracing::warn!(%error, "the report's name list didn't build"))
                .ok()
        };
        Self { names }
    }

    fn line(&self, line: &str) -> String {
        let named = self.replace_names(line);
        redact(&named)
    }

    /// Replace each known name that stands as whole words.
    fn replace_names(&self, line: &str) -> String {
        let Some(names) = &self.names else {
            return line.to_owned();
        };
        let mut spans: Vec<(usize, usize)> = names
            .find_overlapping_iter(line)
            .map(|found| (found.start(), found.end()))
            .filter(|&(start, end)| {
                let before = line[..start].chars().next_back();
                let after = line[end..].chars().next();
                !before.is_some_and(char::is_alphanumeric)
                    && !after.is_some_and(char::is_alphanumeric)
            })
            .collect();
        if spans.is_empty() {
            return line.to_owned();
        }
        spans.sort_unstable();
        let mut output = String::with_capacity(line.len());
        let mut at = 0;
        for (start, end) in spans {
            if end <= at {
                continue;
            }
            if start >= at {
                output.push_str(&line[at..start]);
                output.push_str("<name>");
            }
            at = end;
        }
        output.push_str(&line[at..]);
        output
    }
}

/// Fields whose values name files, folders or what someone typed.
const NAMED_FIELDS: [&str; 8] = [
    "path", "name", "file", "folder", "dir", "title", "query", "pattern",
];

/// Replace what could name a person's files or content without knowing
/// the names: quoted text, paths, file names with an extension, and the
/// values of fields such as `path=` or `name=`.
fn redact(line: &str) -> String {
    redact_words(&replace_quoted(line))
}

/// Replace each quoted span with `<text>`. Inside quotes a backslash
/// escapes the next character, as Debug formatting writes `\"`, and a
/// quote left open runs to the end of the line. An apostrophe inside a
/// word, as in "couldn't", isn't a quote.
fn replace_quoted(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut previous: Option<char> = None;
    while let Some(character) = chars.next() {
        let apostrophe = character == '\''
            && previous.is_some_and(char::is_alphanumeric)
            && chars.peek().is_some_and(|next| next.is_alphanumeric());
        if matches!(character, '"' | '\'' | '`') && !apostrophe {
            while let Some(next) = chars.next() {
                if next == '\\' {
                    chars.next();
                } else if next == character {
                    break;
                }
            }
            output.push_str("<text>");
        } else {
            output.push(character);
        }
        previous = Some(character);
    }
    output
}

/// A `key=value` word: the text up to and including the key, such as
/// `path` or a span's `open{path`, and the value's first word. Keys are
/// letters, digits, `_` and `.`, starting with a letter or `_`.
fn field(word: &str) -> Option<(&str, &str, &str)> {
    let (left, value) = word.split_once('=')?;
    let key = left.rsplit('{').next().unwrap_or(left);
    let is_key = key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    is_key.then_some((left, key, value))
}

/// Whether `word` is, or holds, a path.
fn is_path(word: &str) -> bool {
    word.contains(['/', '\\'])
}

/// Replace, word by word, what could name something. A field's value runs
/// through the following words up to the next `key=` word or the end of
/// the line, since a value such as a path can hold spaces. A named
/// field's value goes whole, and so does any value that holds a path or a
/// file name. Outside a field, a path runs on to the next field too.
fn redact_words(line: &str) -> String {
    let words: Vec<(&str, &str)> = split_words(line);
    let next_field = |from: usize| {
        (from..words.len())
            .find(|&index| field(words[index].1).is_some())
            .unwrap_or(words.len())
    };
    let mut output = String::with_capacity(line.len());
    let mut index = 0;
    while index < words.len() {
        let (space, word) = words[index];
        output.push_str(space);
        if let Some((left, key, first)) = field(word) {
            let end = next_field(index + 1);
            let named = NAMED_FIELDS
                .iter()
                .any(|field| key.to_ascii_lowercase().contains(field));
            let mut value =
                std::iter::once(first).chain(words[index + 1..end].iter().map(|(_, word)| *word));
            if named || value.any(|word| is_path(word) || is_file_name(word)) {
                output.push_str(left);
                output.push_str("=<redacted>");
                index = end;
                continue;
            }
            output.push_str(word);
        } else if is_path(word) {
            output.push_str("<path>");
            index = next_field(index + 1);
            continue;
        } else if is_file_name(word) {
            output.push_str("<file>");
        } else {
            output.push_str(word);
        }
        index += 1;
    }
    // The spaces after the last word.
    output.push_str(&line[line.trim_end().len()..]);
    output
}

/// Each word of `line` with the spaces before it.
fn split_words(line: &str) -> Vec<(&str, &str)> {
    let mut words = Vec::new();
    let mut rest = line;
    loop {
        let start = rest.len() - rest.trim_start().len();
        let (space, after) = rest.split_at(start);
        if after.is_empty() {
            break;
        }
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        let (word, remaining) = after.split_at(end);
        words.push((space, word));
        rest = remaining;
    }
    words
}

/// Whether `word` looks like a file name: a name, a dot and an extension
/// of letters and digits, such as `Plan.md`. Times and versions, which
/// start with a digit, don't count.
fn is_file_name(word: &str) -> bool {
    let word = word.trim_end_matches([',', ';', ':', ')', ']', '.']);
    let Some((stem, extension)) = word.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && !word.starts_with(|c: char| c.is_ascii_digit())
        && (1..=10).contains(&extension.len())
        && extension.chars().all(|c| c.is_ascii_alphanumeric())
        && extension.chars().any(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_hide_paths_file_names_and_quoted_text() {
        let line = r#"[2026-09-28][WARN][local_core::engine::scan] a workspace file could not be read path=Journal/today.md reason="Permission denied (os error 13)""#;
        let redacted = redact(line);
        assert!(!redacted.contains("Journal"));
        assert!(!redacted.contains("today.md"));
        assert!(!redacted.contains("Permission denied"));
        assert!(redacted.contains("path=<redacted>"));
        assert!(redacted.contains("could not be read"));
    }

    #[test]
    fn reports_hide_absolute_and_windows_paths() {
        let redacted = redact(r"open failed for /Users/someone/Notes and C:\Users\someone\Notes");
        assert_eq!(redacted, "open failed for <path>");
        assert_eq!(
            redact(r"open failed for C:\Users\someone\My Notes code=busy"),
            "open failed for <path> code=busy"
        );
    }

    #[test]
    fn a_named_field_hides_every_word_up_to_the_next_field() {
        assert_eq!(
            redact("could not be read path=Divorce Lawyer Meeting/Custody Plan.md error=denied"),
            "could not be read path=<redacted> error=denied"
        );
        assert_eq!(
            redact("rename failed name=Lantern Keeper Diary"),
            "rename failed name=<redacted>"
        );
        assert_eq!(
            redact("open{path=Lantern Keeper}: app_core: opened"),
            "open{path=<redacted>"
        );
    }

    #[test]
    fn a_value_that_holds_a_path_or_a_file_name_goes_whole() {
        assert_eq!(
            redact("didn't open error=failed for /Users/someone/Tax Return Final Draft.pdf"),
            "didn't open error=<redacted>"
        );
        assert_eq!(
            redact("moved from=Secret Project Alpha/Draft.md to=Archive/Box Two code=moved"),
            "moved from=<redacted> to=<redacted> code=moved"
        );
        assert_eq!(
            redact("kept error=Big Plan.md is busy code=busy"),
            "kept error=<redacted> code=busy"
        );
        assert_eq!(
            redact("failed error=No such file or directory (os error 2)"),
            "failed error=No such file or directory (os error 2)"
        );
    }

    #[test]
    fn an_escaped_quote_doesn_t_end_quoted_text() {
        assert_eq!(
            redact(r#"skipped path=Some("/x/My \"Hidden Affair\" notes") code=io"#),
            "skipped path=<redacted> code=io"
        );
        assert_eq!(
            redact(r#"read reason="a \"Hidden Affair\" note" code=io"#),
            "read reason=<text> code=io"
        );
        // A quote left open hides the rest of the line.
        assert_eq!(
            redact(r#"read reason="Hidden \" Affair"#),
            "read reason=<text>"
        );
    }

    #[test]
    fn apostrophes_inside_words_keep_the_message_readable() {
        assert_eq!(
            redact("the index didn't catch up with 'Plan'"),
            "the index didn't catch up with <text>"
        );
    }

    #[test]
    fn times_and_versions_stay_while_file_names_go() {
        assert_eq!(
            redact("2026-10-20T10:00:00.123Z noura 0.2.0 opened Plan.md, then notes.txt."),
            "2026-10-20T10:00:00.123Z noura 0.2.0 opened <file> then <file>"
        );
    }

    #[test]
    fn known_names_go_as_whole_words_in_any_ascii_case() {
        let redactor = Redactor::new(["Okapi Plans".to_owned(), "Tamarind.md".to_owned()]);
        assert_eq!(
            redactor.replace_names("moved okapi plans and TAMARIND, kept Tamarindo"),
            "moved <name> and <name>, kept Tamarindo"
        );
    }
}
