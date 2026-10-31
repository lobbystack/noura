//! Every row of sync-v2.md §30 marked as detected maps to named tests that
//! see the error the row names (seam 4). `fixtures/threat-tests.json` holds
//! the map; this test reads §30 and the test sources themselves, lists each
//! row with its tests, and fails on a row without one. The scheduled job
//! then runs the named tests: scripts/threat-tests.sh prints them.
//!
//! A row marked `harmless` needs no test, since nothing there is detected.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

const MAP: &str = "tests/fixtures/threat-tests.json";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    threat: String,
    /// `file::function`, in this crate's `tests` folder.
    tests: Vec<String>,
    /// What the tests must see beyond the errors the row names, for a row
    /// that names none or names evidence rather than an error.
    #[serde(default)]
    sees: Vec<String>,
}

/// One row of §30's detection table.
struct Row {
    threat: String,
    detected: String,
    /// The `sync_*` error codes the row names.
    errors: BTreeSet<String>,
}

fn crate_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn error_codes(text: &str) -> BTreeSet<String> {
    let mut codes = BTreeSet::new();
    for part in text.split('`') {
        let word = part.trim();
        if word.starts_with("sync_")
            && word
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
        {
            codes.insert(word.to_owned());
        }
    }
    codes
}

/// The rows of the table that follows `### What a malicious server or relay
/// can do` in §30.
fn rows(spec: &str) -> Vec<Row> {
    let table = spec
        .split("### What a malicious server or relay can do")
        .nth(1)
        .expect("§30's table");
    let table = table.split("\n### ").next().expect("the section");
    table
        .lines()
        .filter(|line| line.starts_with("| ") && !line.starts_with("| The server |"))
        .map(|line| {
            let mut cells = line.trim_matches('|').splitn(3, " | ");
            let threat = cells.next().expect("a threat").trim().to_owned();
            let detected = cells.next().expect("a verdict").trim().to_owned();
            let how = cells.next().expect("how").trim();
            Row {
                errors: error_codes(how),
                threat,
                detected,
            }
        })
        .collect()
}

/// The block that follows byte `at`, from its first `{` to the matching `}`.
fn block(source: &str, at: usize) -> &str {
    let Some(start) = source[at..].find('{').map(|offset| at + offset) else {
        return "";
    };
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[start..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=start + offset];
                }
            }
            _ => {}
        }
    }
    &source[start..]
}

/// One test source: every function's body, and which are tests.
struct Source {
    functions: BTreeMap<String, String>,
    tests: BTreeSet<String>,
}

fn read_source(path: &Path) -> Source {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut functions = BTreeMap::new();
    let mut tests = BTreeSet::new();
    let mut attribute = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test") {
            attribute = true;
        }
        let named = trimmed
            .strip_prefix("pub ")
            .unwrap_or(trimmed)
            .strip_prefix("async ")
            .unwrap_or(trimmed.strip_prefix("pub ").unwrap_or(trimmed))
            .strip_prefix("fn ");
        if let Some(rest) = named {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            functions.insert(
                name.clone(),
                block(&text, offset + line.len() - rest.len()).to_owned(),
            );
            if attribute {
                tests.insert(name);
            }
            attribute = false;
        }
        offset += line.len();
    }
    Source { functions, tests }
}

impl Source {
    /// The test's body and those of the file's functions it calls.
    fn reach(&self, test: &str) -> String {
        let mut seen = BTreeSet::from([test.to_owned()]);
        let mut waiting = vec![test.to_owned()];
        let mut reached = String::new();
        while let Some(name) = waiting.pop() {
            let Some(body) = self.functions.get(&name) else {
                continue;
            };
            reached.push_str(body);
            for called in self.functions.keys() {
                if !seen.contains(called) && body.contains(&format!("{called}(")) {
                    seen.insert(called.clone());
                    waiting.push(called.clone());
                }
            }
        }
        reached
    }
}

#[test]
fn every_detected_row_of_section_30_has_a_test_that_sees_its_error() {
    let spec =
        std::fs::read_to_string(crate_path("../../docs/workspace-format/sync-v2.md")).expect("§30");
    let rows = rows(&spec);
    assert!(rows.len() > 30, "found {} rows", rows.len());
    let entries: Vec<Entry> =
        serde_json::from_slice(&std::fs::read(crate_path(MAP)).expect("the map")).expect("JSON");

    let mut sources: BTreeMap<String, Source> = BTreeMap::new();
    let mut problems = Vec::new();
    for row in &rows {
        let harmless = row.detected == "harmless";
        let entry = entries.iter().find(|e| e.threat == row.threat);
        let Some(entry) = entry else {
            if !harmless {
                problems.push(format!("no test for: {}", row.threat));
            }
            continue;
        };
        println!("{} ({})", row.threat, row.detected);
        let mut reached = String::new();
        for name in &entry.tests {
            println!("    {name}");
            let Some((file, test)) = name.split_once("::") else {
                problems.push(format!("{name}: not file::function"));
                continue;
            };
            let source = sources
                .entry(file.to_owned())
                .or_insert_with(|| read_source(&crate_path(&format!("tests/{file}.rs"))));
            if source.tests.contains(test) {
                reached.push_str(&source.reach(test));
            } else {
                problems.push(format!("{name}: no such test"));
            }
        }
        for error in row.errors.iter().chain(&entry.sees) {
            if !reached.contains(&format!("\"{error}\"")) {
                problems.push(format!("{}: no test sees {error}", row.threat));
            }
        }
        if !harmless && row.errors.is_empty() && entry.tests.is_empty() {
            problems.push(format!("no test for: {}", row.threat));
        }
    }
    for entry in &entries {
        if !rows.iter().any(|row| row.threat == entry.threat) {
            problems.push(format!("not a row of §30: {}", entry.threat));
        }
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}
