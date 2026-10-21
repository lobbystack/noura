//! Quick open: find any file in the tree by name from the keyboard.
//!
//! A file matches when its name holds every typed word's letters in
//! order, or failing that, its folder and name together do. Name matches
//! come before folder matches, an exact name before a name that starts
//! with what you typed, and that before other name matches. Within each
//! group, `nucleo-matcher` ranks consecutive letters and word starts above
//! scattered letters, ignoring case and accents.

use std::cmp::{Ordering, Reverse};

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

use crate::{
    EntryKind,
    files::{parent_of, shown_name, sort_key},
};

/// Quick open lists at most this many matches.
pub const QUICK_OPEN_LIMIT: usize = 50;

/// One file quick open lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickOpenMatch {
    /// The name the file tree shows: Markdown files appear without `.md`.
    pub name: String,
    /// The folder that holds the file, or `""` at the top of the workspace.
    pub folder: String,
    pub relative_path: String,
}

/// Where a file's match ranks, best last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    /// The folder and name together hold the letters.
    Folder,
    /// The name holds the letters.
    Name,
    /// The name starts with what you typed.
    Prefix,
    /// The name is what you typed.
    Exact,
}

struct Ranked {
    group: Group,
    score: u32,
    found: QuickOpenMatch,
}

impl Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        (Reverse(self.group), Reverse(self.score))
            .cmp(&(Reverse(other.group), Reverse(other.score)))
            // A shorter name that matches as well is the closer match.
            .then_with(|| self.found.name.len().cmp(&other.found.name.len()))
            .then_with(|| self.found.relative_path.cmp(&other.found.relative_path))
    }
}

/// The files among `paths` that match `query`, best first. Nothing typed
/// matches nothing.
pub(crate) fn rank<'a>(
    paths: impl IntoIterator<Item = &'a str>,
    query: &str,
) -> Vec<QuickOpenMatch> {
    let typed = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if typed.is_empty() {
        return Vec::new();
    }
    let folded = sort_key(&typed);
    let pattern = Pattern::new(
        &typed,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    let mut names = Matcher::new(Config::DEFAULT);
    let mut paths_matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buffer = Vec::new();
    let mut ranked: Vec<Ranked> = paths
        .into_iter()
        .filter_map(|path| {
            let file_name = path.rsplit('/').next().unwrap_or(path);
            let name = shown_name(file_name, EntryKind::File);
            let folder = parent_of(path).unwrap_or("");
            let (group, score) = match pattern.score(Utf32Str::new(&name, &mut buffer), &mut names)
            {
                Some(score) => {
                    let name = sort_key(&name);
                    let group = if name == folded {
                        Group::Exact
                    } else if name.starts_with(&folded) {
                        Group::Prefix
                    } else {
                        Group::Name
                    };
                    (group, score)
                }
                None if folder.is_empty() => return None,
                None => {
                    let shown = format!("{folder}/{name}");
                    let score =
                        pattern.score(Utf32Str::new(&shown, &mut buffer), &mut paths_matcher)?;
                    (Group::Folder, score)
                }
            };
            Some(Ranked {
                group,
                score,
                found: QuickOpenMatch {
                    name,
                    folder: folder.to_owned(),
                    relative_path: path.to_owned(),
                },
            })
        })
        .collect();
    ranked.sort_by(Ranked::cmp);
    ranked.truncate(QUICK_OPEN_LIMIT);
    ranked.into_iter().map(|ranked| ranked.found).collect()
}
