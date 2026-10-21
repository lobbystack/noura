//! Where the links and embeds in a note lead. A target resolves the way
//! Obsidian and today's app resolve it: relative to the note, then from the
//! workspace root, then by file name anywhere in the workspace, ignoring
//! case and preferring the note's folder and then the shortest path. `Plan`
//! finds `Plan.md`. Resolution reads the newest listing of the workspace's
//! files, which holds nothing outside the workspace, so a link never leads
//! out of it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use local_core::WorkspaceEngine;

use crate::FileTree;

/// How much of an embedded note's text an embed shows, in characters.
pub const EMBED_PREVIEW_CHARS: usize = 800;

/// The open workspace's files as its newest listing shows them. The
/// session updates it with each listing, and every note's links read it.
#[derive(Clone, Default)]
pub(crate) struct LinkFiles(Arc<RwLock<Arc<LinkIndex>>>);

impl LinkFiles {
    /// Take the files `tree` lists.
    pub(crate) fn list(&self, tree: &FileTree) {
        let mut current = self
            .0
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut index = LinkIndex::new(tree.file_paths());
        index.seq = current.seq + 1;
        *current = Arc::new(index);
    }

    fn get(&self) -> Arc<LinkIndex> {
        self.0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// The workspace's files, by path and by lowercase file name, so a note
/// with many links resolves them quickly in a large workspace, with the
/// notes embedded since this listing.
#[derive(Default)]
pub(crate) struct LinkIndex {
    /// Counts the listings so far.
    seq: u64,
    paths: HashSet<String>,
    by_name: HashMap<String, Vec<String>>,
    /// What each note embedded since this listing shows. A change to any
    /// file makes a new listing, which starts over.
    embeds: Mutex<HashMap<String, Option<NoteEmbed>>>,
}

impl LinkIndex {
    pub(crate) fn new<'a>(files: impl IntoIterator<Item = &'a str>) -> Self {
        let mut index = Self::default();
        for file in files {
            let name = file.rsplit('/').next().unwrap_or(file).to_lowercase();
            index.by_name.entry(name).or_default().push(file.to_owned());
            index.paths.insert(file.to_owned());
        }
        index
    }
}

/// What an embedded note shows: its title, which is its file name, and
/// the first [`EMBED_PREVIEW_CHARS`] characters of its text. A note of
/// Noura's own shows the text after its frontmatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEmbed {
    pub title: String,
    pub text: String,
}

/// What the links in one note lead to. A note's editor holds one and asks
/// it about each link and embed it draws; it reads the workspace's newest
/// listing, so links follow files as they appear, move and go.
#[derive(Clone)]
pub struct NoteLinks {
    source: String,
    files: LinkFiles,
    engine: Arc<WorkspaceEngine>,
}

impl NoteLinks {
    pub(crate) fn new(source: &str, files: LinkFiles, engine: Arc<WorkspaceEngine>) -> Self {
        Self {
            source: source.to_owned(),
            files,
            engine,
        }
    }

    /// The workspace path of the note these links belong to.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Changes with each listing of the workspace's files, after which any
    /// answer may differ.
    pub fn seq(&self) -> u64 {
        self.files.get().seq
    }

    /// The workspace file `target` leads to, or `None` when it names no
    /// file in the workspace. A `#heading` part is ignored.
    pub fn resolve(&self, target: &str) -> Option<String> {
        resolve_link_path(&self.source, target, &self.files.get())
    }

    /// What an `![[target]]` embed shows when `target` names a Markdown
    /// note, or `None` when it names anything else or nothing. It reads
    /// the note and writes nothing.
    pub fn embed(&self, target: &str) -> Option<NoteEmbed> {
        let index = self.files.get();
        let path = resolve_link_path(&self.source, target, &index)?;
        if !crate::notes::is_markdown(&path) {
            return None;
        }
        let mut embeds = index
            .embeds
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        embeds
            .entry(path.clone())
            .or_insert_with(|| self.read_embed(&path))
            .clone()
    }

    fn read_embed(&self, path: &str) -> Option<NoteEmbed> {
        let read = self.engine.read_raw_markdown(path).ok()?;
        let text = match local_core::parse_markdown(path, read.body.as_bytes()) {
            local_core::ParsedMarkdown::Managed(_) => {
                let start = local_core::frontmatter_end(&read.body).unwrap_or(0);
                &read.body[start..]
            }
            _ => read.body.as_str(),
        };
        Some(NoteEmbed {
            title: crate::files::shown_name(
                path.rsplit('/').next().unwrap_or(path),
                crate::EntryKind::File,
            ),
            text: text.chars().take(EMBED_PREVIEW_CHARS).collect(),
        })
    }
}

/// `target` without its `#heading` or `#^block` part, percent-decoded as
/// Markdown links write spaces. A target that doesn't decode stays as
/// written.
fn link_path(target: &str) -> String {
    let raw = target.split('#').next().unwrap_or_default().trim();
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
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rfind('.').is_some_and(|dot| {
        let extension = &name[dot + 1..];
        (1..=8).contains(&extension.len()) && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
    })
}

/// Whether `path` starts with a URL scheme, as `https:` or `mailto:` do.
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

/// The paths a link to `candidate` may mean: `candidate` and
/// `candidate.md`, the one with an extension first.
fn variants(candidate: &str) -> [String; 2] {
    let with_md = format!("{candidate}.md");
    if has_extension(candidate) {
        [candidate.to_owned(), with_md]
    } else {
        [with_md, candidate.to_owned()]
    }
}

/// The file in `files` that `target`, written in the note at `source`,
/// leads to.
pub(crate) fn resolve_link_path(source: &str, target: &str, files: &LinkIndex) -> Option<String> {
    let path = link_path(target);
    if path.is_empty() || has_scheme(&path) {
        return None;
    }
    let known = |candidate: &str| files.paths.contains(candidate);
    let source_folder = folder_of(source);
    let direct = if path.starts_with('/') {
        vec![join_path("", &path)]
    } else {
        vec![join_path(source_folder, &path), join_path("", &path)]
    };
    for candidate in direct.into_iter().flatten() {
        if let Some(found) = variants(&candidate)
            .into_iter()
            .find(|variant| known(variant))
        {
            return Some(found);
        }
    }
    let wanted = variants(path.trim_start_matches('/')).map(|variant| variant.to_lowercase());
    wanted
        .iter()
        .filter_map(|variant| {
            files
                .by_name
                .get(variant.rsplit('/').next().unwrap_or(variant))
        })
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

#[cfg(test)]
mod tests {
    use super::{has_extension, join_path, link_path};

    #[test]
    fn paths_leave_headings_and_decode_spaces() {
        assert_eq!(link_path("Old%20notes.md#Top"), "Old notes.md");
        assert_eq!(link_path("100%"), "100%");
        assert_eq!(link_path("  Plan  "), "Plan");
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
}
