//! Links between workspace files, as ADR 0005 decides: every link is
//! `[[Title]]`, or `[[Folder/Title]]` when two files share the title, in a
//! body and in a property alike. This module finds links in text, says
//! which file a link leads to, writes the link Noura uses for a file, and
//! rewrites links after a file moves. It reads no file: callers pass the
//! workspace's file paths.
//!
//! A link leads where the editor sends it: relative to the linking file,
//! then from the top of the workspace, then by file name anywhere in the
//! workspace, ignoring case and preferring the linking file's folder and
//! then the shortest path. `Plan` finds `Plan.md`.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// The workspace's file paths, ready to resolve links against.
#[derive(Debug, Clone, Default)]
pub struct LinkTargets {
    paths: HashSet<String>,
    /// Each file by its lowercase file name, extension included.
    by_name: HashMap<String, Vec<String>>,
    /// How many files show each title, by lowercase title: a Markdown
    /// file's name without `.md`, any other file's whole name.
    titles: HashMap<String, usize>,
}

impl LinkTargets {
    /// Every file in `paths`, workspace-relative with `/` between parts.
    pub fn new<'a>(paths: impl IntoIterator<Item = &'a str>) -> Self {
        let mut targets = Self::default();
        for path in paths {
            targets.insert(path);
        }
        targets
    }

    /// Add the file at `path`.
    pub fn insert(&mut self, path: &str) {
        if !self.paths.insert(path.to_owned()) {
            return;
        }
        let name = file_name(path).to_lowercase();
        self.by_name.entry(name).or_default().push(path.to_owned());
        *self
            .titles
            .entry(title_of(path).to_lowercase())
            .or_default() += 1;
    }

    /// Whether `path` is one of the files.
    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// The file that a link to `target`, written in the file at `source`,
    /// leads to. `target` is what stands between `[[` and `]]`, with or
    /// without a heading after `#` or a label after `|`.
    pub fn resolve(&self, source: &str, target: &str) -> Option<String> {
        let path = link_path(target);
        if path.is_empty() || has_scheme(&path) {
            return None;
        }
        let source_folder = folder_of(source);
        let direct = if path.starts_with('/') {
            vec![join_path("", &path)]
        } else {
            vec![join_path(source_folder, &path), join_path("", &path)]
        };
        for candidate in direct.into_iter().flatten() {
            if let Some(found) = variants(&candidate)
                .into_iter()
                .find(|variant| self.paths.contains(variant))
            {
                return Some(found);
            }
        }
        let wanted = variants(path.trim_start_matches('/')).map(|variant| variant.to_lowercase());
        wanted
            .iter()
            .filter_map(|variant| self.by_name.get(file_name(variant)))
            .flatten()
            .filter(|file| {
                let lower = file.to_lowercase();
                wanted
                    .iter()
                    .any(|variant| lower == *variant || lower.ends_with(&format!("/{variant}")))
            })
            .min_by(|left, right| {
                let rank = |file: &str| usize::from(folder_of(file) != source_folder);
                rank(left)
                    .cmp(&rank(right))
                    .then(left.split('/').count().cmp(&right.split('/').count()))
                    .then(left.to_lowercase().cmp(&right.to_lowercase()))
                    .then(left.cmp(right))
            })
            .cloned()
    }

    /// What Noura writes between `[[` and `]]` to link the file at `path`
    /// from the file at `source`: its title when no other file shares it,
    /// otherwise its path from the top of the workspace without `.md`.
    /// When that leads elsewhere from `source`, the path with `/` before
    /// it, then the path with its extension.
    pub fn link_for(&self, source: &str, path: &str) -> String {
        let title = title_of(path);
        let shared = self
            .titles
            .get(&title.to_lowercase())
            .is_some_and(|count| *count > 1);
        let without_md = path.strip_suffix(".md").unwrap_or(path);
        let mut links = vec![
            without_md.to_owned(),
            format!("/{without_md}"),
            path.to_owned(),
            format!("/{path}"),
        ];
        if !shared {
            links.insert(0, title.to_owned());
        }
        links
            .iter()
            .find(|link| self.resolve(source, link).as_deref() == Some(path))
            .unwrap_or(&links[0])
            .clone()
    }
}

/// A link's place in a body: the bytes of its target, between `[[` and
/// the `#`, `|` or `]]` after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyLink {
    /// The byte range of the target in the text.
    pub target: std::ops::Range<usize>,
}

/// The `[[…]]` and `![[…]]` links in Markdown `text`, in order. Links in
/// fenced code blocks and in inline code don't count.
pub fn body_links(text: &str) -> Vec<BodyLink> {
    let mut links = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let run = |mark: u8| trimmed.bytes().take_while(|byte| *byte == mark).count();
        if indent < 4 {
            match fence {
                Some((mark, length)) => {
                    if run(mark) >= length && trimmed[run(mark)..].trim().is_empty() {
                        fence = None;
                    }
                    continue;
                }
                None => {
                    for mark in *b"`~" {
                        if run(mark) >= 3 {
                            fence = Some((mark, run(mark)));
                        }
                    }
                    if fence.is_some() {
                        continue;
                    }
                }
            }
        } else if fence.is_some() {
            continue;
        }
        line_links(line, start, &mut links);
    }
    links
}

/// The links on one line that starts at byte `start` of the text.
fn line_links(line: &str, start: usize, links: &mut Vec<BodyLink>) {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'`' => {
                let ticks = bytes[index..]
                    .iter()
                    .take_while(|byte| **byte == b'`')
                    .count();
                let closing = "`".repeat(ticks);
                match line[index + ticks..].find(&closing) {
                    Some(end) => index += ticks + end + ticks,
                    None => index += ticks,
                }
            }
            b'[' if bytes.get(index + 1) == Some(&b'[') => {
                let inner_start = index + 2;
                let Some(length) = line[inner_start..].find("]]") else {
                    return;
                };
                let inner = &line[inner_start..inner_start + length];
                if inner.contains("[[") || inner.trim().is_empty() {
                    index += 2;
                    continue;
                }
                let target_length = inner.find(['#', '|', '^']).unwrap_or(inner.len());
                let target = &inner[..target_length];
                let leading = target.len() - target.trim_start().len();
                let trimmed = target.trim();
                if !trimmed.is_empty() {
                    let from = start + inner_start + leading;
                    links.push(BodyLink {
                        target: from..from + trimmed.len(),
                    });
                }
                index = inner_start + length + 2;
            }
            _ => index += 1,
        }
    }
}

/// `text` with the target of each link that `rename` gives a new target
/// for replaced by it, or `None` when no link changes. Headings, labels
/// and everything else keep their bytes.
pub fn rewrite_body_links(
    text: &str,
    mut rename: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut changed = false;
    for link in body_links(text) {
        let target = &text[link.target.clone()];
        let Some(new) = rename(target) else {
            continue;
        };
        if new == target {
            continue;
        }
        out.push_str(&text[copied..link.target.start]);
        out.push_str(&new);
        copied = link.target.end;
        changed = true;
    }
    changed.then(|| {
        out.push_str(&text[copied..]);
        out
    })
}

/// The target of a property value that is a link, `[[Title]]` or
/// `[[Folder/Title]]`, with a heading or a label allowed after it. Spaces
/// around the brackets don't count. Any other value is no link.
pub fn property_link(value: &str) -> Option<&str> {
    let inner = value.trim().strip_prefix("[[")?.strip_suffix("]]")?;
    if inner.contains("[[") || inner.contains("]]") {
        return None;
    }
    let target = inner[..inner.find(['#', '|', '^']).unwrap_or(inner.len())].trim();
    (!target.is_empty()).then_some(target)
}

/// The link Noura writes in a property or a body for `target`, such as
/// `[[Garden]]`.
pub fn link(target: &str) -> String {
    format!("[[{target}]]")
}

/// `value` with each link in it that `rename` gives a new target for
/// rewritten: a link string, or a list holding link strings. `None` when
/// nothing changes. A heading or a label after the target stays.
pub fn rewrite_property_links(
    value: &Value,
    rename: &mut impl FnMut(&str) -> Option<String>,
) -> Option<Value> {
    match value {
        Value::String(text) => {
            let target = property_link(text)?;
            let new = rename(target)?;
            if new == target {
                return None;
            }
            let at = text.find(target)?;
            Some(Value::String(format!(
                "{}{new}{}",
                &text[..at],
                &text[at + target.len()..]
            )))
        }
        Value::Array(items) => {
            let mut changed = false;
            let items = items
                .iter()
                .map(|item| match rewrite_property_links(item, rename) {
                    Some(new) => {
                        changed = true;
                        new
                    }
                    None => item.clone(),
                })
                .collect();
            changed.then_some(Value::Array(items))
        }
        _ => None,
    }
}

/// The link targets a property value holds: a link string, or each link
/// string in a list.
pub fn property_links(value: &Value) -> Vec<&str> {
    match value {
        Value::String(text) => property_link(text).into_iter().collect(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().and_then(property_link))
            .collect(),
        _ => Vec::new(),
    }
}

/// The lowercase file name a link target names, with `.md` added when it
/// names no extension, as the index looks links up by: `Plan` and
/// `Folder/plan.md` both give `plan.md`.
pub fn link_name(target: &str) -> String {
    let path = link_path(target);
    let name = file_name(&path);
    if has_extension(name) {
        name.to_lowercase()
    } else {
        format!("{}.md", name.to_lowercase())
    }
}

/// The lowercase file names a link to `path` could be looked up by: its
/// own name and, for `Report.pdf.md`, `report.pdf` too.
pub fn names_of(path: &str) -> Vec<String> {
    let name = file_name(path).to_lowercase();
    let mut names = vec![name.clone()];
    if let Some(stem) = name.strip_suffix(".md")
        && has_extension(stem)
    {
        names.push(stem.to_owned());
    }
    names
}

/// A link's target without its heading, its label or its block, with
/// `%20` and the like decoded and the spaces around it trimmed.
pub fn link_path(target: &str) -> String {
    let raw = target
        .split(['#', '|', '^'])
        .next()
        .unwrap_or_default()
        .trim();
    percent_decode(raw).unwrap_or_else(|| raw.to_owned())
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A file's title as links name it: a Markdown file's name without `.md`,
/// any other file's whole name.
fn title_of(path: &str) -> &str {
    let name = file_name(path);
    name.strip_suffix(".md").unwrap_or(name)
}

fn folder_of(path: &str) -> &str {
    path.rfind('/').map_or("", |cut| &path[..cut])
}

/// `path` joined onto `folder` with `.` and `..` resolved, or `None` when
/// it leaves the workspace.
fn join_path(folder: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = folder.split('/').filter(|part| !part.is_empty()).collect();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            segment => parts.push(segment),
        }
    }
    Some(parts.join("/"))
}

/// Whether the last part of `path` has an extension of up to 8 letters
/// and digits.
fn has_extension(path: &str) -> bool {
    let name = file_name(path);
    name.rfind('.').is_some_and(|dot| {
        let extension = &name[dot + 1..];
        (1..=8).contains(&extension.len()) && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
    })
}

fn has_scheme(path: &str) -> bool {
    let Some(colon) = path.find(':') else {
        return false;
    };
    let scheme = &path[..colon];
    scheme
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-'))
}

fn variants(candidate: &str) -> [String; 2] {
    let with_md = format!("{candidate}.md");
    if has_extension(candidate) {
        [candidate.to_owned(), with_md]
    } else {
        [with_md, candidate.to_owned()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(text: &str) -> Vec<&str> {
        body_links(text)
            .into_iter()
            .map(|link| &text[link.target])
            .collect()
    }

    #[test]
    fn bodies_link_by_title_outside_code() {
        let text = "See [[Garden]] and ![[Plan#Steps|the plan]].\n\
                    `[[Not a link]]` and \\[[escaped]]\n\
                    ```\n[[In a fence]]\n```\n\
                    [[ Spaced ]] [[]] [[a [[b]]\n";
        assert_eq!(targets(text), ["Garden", "Plan", "Spaced", "b"]);
    }

    #[test]
    fn rewriting_keeps_headings_labels_and_every_other_byte() {
        let text = "[[Garden]], [[garden#Beds|beds]] and [[Kitchen]]\r\n";
        let rewritten = rewrite_body_links(text, |target| {
            target
                .eq_ignore_ascii_case("garden")
                .then(|| "Backyard".to_owned())
        });
        assert_eq!(
            rewritten.as_deref(),
            Some("[[Backyard]], [[Backyard#Beds|beds]] and [[Kitchen]]\r\n")
        );
        assert_eq!(rewrite_body_links(text, |_| None), None);
    }

    #[test]
    fn properties_hold_links_alone_or_in_lists() {
        assert_eq!(property_link("[[Garden]]"), Some("Garden"));
        assert_eq!(
            property_link(" [[Projects/Garden|G]] "),
            Some("Projects/Garden")
        );
        assert_eq!(property_link("Garden"), None);
        assert_eq!(property_link("[[]]"), None);
        assert_eq!(property_link("[[a]] and [[b]]"), None);
        let list = serde_json::json!(["[[One]]", "[[Two]]", 3]);
        assert_eq!(property_links(&list), ["One", "Two"]);
        let renamed = rewrite_property_links(&list, &mut |target| {
            (target == "Two").then(|| "Deux".to_owned())
        });
        assert_eq!(renamed, Some(serde_json::json!(["[[One]]", "[[Deux]]", 3])));
    }

    #[test]
    fn links_resolve_as_the_editor_resolves_them() {
        let targets = LinkTargets::new([
            "Garden.md",
            "Projects/Garden/Garden.md",
            "Notes/Plan.md",
            "Report.pdf",
        ]);
        assert_eq!(
            targets.resolve("Notes/x.md", "garden").as_deref(),
            Some("Garden.md")
        );
        assert_eq!(
            targets
                .resolve("Projects/Garden/Notes.md", "Garden")
                .as_deref(),
            Some("Projects/Garden/Garden.md")
        );
        assert_eq!(
            targets.resolve("x.md", "Projects/Garden/Garden").as_deref(),
            Some("Projects/Garden/Garden.md")
        );
        assert_eq!(
            targets.resolve("x.md", "Plan").as_deref(),
            Some("Notes/Plan.md")
        );
        assert_eq!(
            targets.resolve("x.md", "Report.pdf").as_deref(),
            Some("Report.pdf")
        );
        assert_eq!(targets.resolve("x.md", "https://example.com"), None);
        assert_eq!(targets.resolve("x.md", "Missing"), None);
    }

    #[test]
    fn noura_links_by_title_and_by_path_when_two_files_share_it() {
        let targets = LinkTargets::new(["Garden.md", "Projects/Garden/Garden.md", "Notes/Plan.md"]);
        assert_eq!(targets.link_for("x.md", "Notes/Plan.md"), "Plan");
        assert_eq!(
            targets.link_for("x.md", "Projects/Garden/Garden.md"),
            "Projects/Garden/Garden"
        );
        assert_eq!(targets.link_for("x.md", "Garden.md"), "Garden");
    }

    #[test]
    fn links_behave_as_the_shared_fixtures_say() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../docs/workspace-format/fixtures/conformance-v1.json"
        ))
        .unwrap();
        let links = &fixture["links"];
        let files: Vec<&str> = links["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file.as_str().unwrap())
            .collect();
        let targets = LinkTargets::new(files);
        let text = |case: &Value, key: &str| case[key].as_str().unwrap().to_owned();
        for case in links["resolve"].as_array().unwrap() {
            assert_eq!(
                targets.resolve(&text(case, "source"), &text(case, "target")),
                case["path"].as_str().map(str::to_owned),
                "{}",
                case["name"]
            );
        }
        for case in links["link_for"].as_array().unwrap() {
            assert_eq!(
                link(&targets.link_for(&text(case, "source"), &text(case, "path"))),
                text(case, "link"),
                "{}",
                case["name"]
            );
        }
        for case in links["body_links"].as_array().unwrap() {
            let body = text(case, "body");
            let found: Vec<Value> = body_links(&body)
                .into_iter()
                .map(|link| Value::String(body[link.target].to_owned()))
                .collect();
            assert_eq!(Value::Array(found), case["targets"], "{}", case["name"]);
        }
        for case in links["rewrites"].as_array().unwrap() {
            let from = text(case, "from");
            let rewritten = rewrite_body_links(&text(case, "body"), |target| {
                target.eq_ignore_ascii_case(&from).then(|| text(case, "to"))
            });
            assert_eq!(rewritten, Some(text(case, "expected")), "{}", case["name"]);
        }
    }

    #[test]
    fn joining_never_climbs_out_of_the_workspace() {
        assert_eq!(join_path("a/b", "../c").as_deref(), Some("a/c"));
        assert_eq!(join_path("a", "../../c"), None);
        assert_eq!(join_path("", "./x/./y").as_deref(), Some("x/y"));
    }

    #[test]
    fn extensions_are_short_letters_and_digits() {
        assert!(has_extension("a/diagram.png"));
        assert!(has_extension("x.markdown"));
        assert!(!has_extension("v1.2 notes"));
        assert!(!has_extension("folder.d/Plan"));
    }

    #[test]
    fn link_names_are_the_file_names_the_index_looks_up() {
        assert_eq!(link_name("Plan"), "plan.md");
        assert_eq!(link_name("Folder/Plan.md#Top"), "plan.md");
        assert_eq!(link_name("Report.pdf"), "report.pdf");
        assert_eq!(names_of("Notes/Plan.md"), ["plan.md"]);
        assert_eq!(names_of("Report.pdf.md"), ["report.pdf.md", "report.pdf"]);
    }
}
