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
const NAMED_FIELDS: [&str; 7] = ["path", "name", "file", "folder", "dir", "title", "query"];

/// Replace what could name a person's files or content without knowing
/// the names: quoted text, absolute and relative paths, file names with
/// an extension, and fields such as `path=` or `name=`.
fn redact(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut word = String::new();
    let mut chars = line.chars().peekable();
    let mut previous: Option<char> = None;
    while let Some(character) = chars.next() {
        // An apostrophe inside a word, as in "couldn't", isn't a quote.
        let apostrophe = character == '\''
            && previous.is_some_and(char::is_alphanumeric)
            && chars.peek().is_some_and(|next| next.is_alphanumeric());
        if matches!(character, '"' | '\'' | '`') && !apostrophe {
            flush(&mut word, &mut output);
            for next in chars.by_ref() {
                if next == character {
                    break;
                }
            }
            output.push_str("<text>");
        } else if character.is_whitespace() {
            flush(&mut word, &mut output);
            output.push(character);
        } else {
            word.push(character);
        }
        previous = Some(character);
    }
    flush(&mut word, &mut output);
    output
}

/// Add one word of a log line to `output`, replaced when it could name
/// something.
fn flush(word: &mut String, output: &mut String) {
    if word.is_empty() {
        return;
    }
    let path_like = word.contains('/') || word.contains('\\');
    match word.split_once('=') {
        Some((key, value))
            if path_like
                || is_file_name(value)
                || NAMED_FIELDS
                    .iter()
                    .any(|field| key.to_ascii_lowercase().contains(field)) =>
        {
            output.push_str(key);
            output.push_str("=<redacted>");
        }
        _ if path_like => output.push_str("<path>"),
        _ if is_file_name(word) => output.push_str("<file>"),
        _ => output.push_str(word),
    }
    word.clear();
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
        assert!(!redacted.contains("someone"));
        assert_eq!(redacted.matches("<path>").count(), 2);
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
