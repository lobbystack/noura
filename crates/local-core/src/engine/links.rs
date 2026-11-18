//! Links by title, kept current (ADR 0005). After a file moves or gets a
//! new title, in Noura or in another app, the engine rewrites every link
//! that led to it, in bodies and in properties, so each still leads
//! there. It reads each file it rewrites and writes only if the file
//! still holds what it read, so an edit that lands meanwhile wins and the
//! file shows up among the workspace's problems instead.
//!
//! Links are looked up among the workspace's Markdown files, which the
//! index lists: a move of another kind of file, such as a PDF, rewrites
//! nothing.

use std::collections::{BTreeMap, HashMap, HashSet};

use workspace_format::links::{
    LinkTargets, link, names_of, rewrite_body_links, rewrite_property_links,
};

use super::*;

/// Test hook: runs with each file's path before a link rewrite writes it.
#[cfg(any(test, feature = "test-hooks"))]
pub type LinkRewriteHook = Box<dyn Fn(&str) + Send + Sync>;

/// A file whose links to a moved file Noura couldn't update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleLink {
    /// The link target the file still holds.
    pub target: String,
    /// Where the file it led to went.
    pub moved_to: String,
}

impl WorkspaceEngine {
    /// The workspace's Markdown files, ready to resolve links against.
    pub fn link_targets(&self) -> Result<LinkTargets> {
        self.reconcile_if_needed()?;
        let paths = self
            .index
            .lock()
            .map_err(|_| lock_error("links_read"))?
            .markdown_paths()?;
        Ok(LinkTargets::new(paths.iter().map(String::as_str)))
    }

    /// The file that `link`, written in the file at `source` or at the top
    /// of the workspace when `source` is `None`, leads to. `link` may be
    /// written with its brackets, `[[Garden]]`, or without, `Garden`.
    pub fn resolve_link(&self, source: Option<&str>, link: &str) -> Result<Option<String>> {
        let target = workspace_format::links::property_link(link).unwrap_or(link);
        Ok(self.link_targets()?.resolve(source.unwrap_or(""), target))
    }

    /// The link Noura writes for the file at `path` from the file at
    /// `source`, or from the top of the workspace: `[[Title]]`, or
    /// `[[Folder/Title]]` when another file shares the title.
    pub fn link_to_path(&self, source: Option<&str>, path: &str) -> Result<String> {
        Ok(link(
            &self.link_targets()?.link_for(source.unwrap_or(""), path),
        ))
    }

    /// The link Noura writes for the object with the stable ID `id`.
    pub fn link_to_object(&self, id: &str) -> Result<String> {
        let path = self.read_object(id)?.relative_path;
        self.link_to_path(None, &path)
    }

    /// The Markdown files with a link that leads to the file at `path`, in
    /// path order. The index finds them, so a rebuilt index finds the same.
    pub fn backlinks(&self, path: &str) -> Result<Vec<String>> {
        let targets = self.link_targets()?;
        let sources = self
            .index
            .lock()
            .map_err(|_| lock_error("links_read"))?
            .link_sources(&names_of(path))?;
        Ok(sources
            .into_iter()
            .filter(|(source, _, links)| {
                source != path
                    && links
                        .iter()
                        .any(|target| targets.resolve(source, target).as_deref() == Some(path))
            })
            .map(|(source, ..)| source)
            .collect())
    }

    /// Test hook: run `hook` with each file's path before a link rewrite
    /// writes it, as another app's edit would land.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_link_rewrite_hook(&self, hook: Option<LinkRewriteHook>) {
        if let Ok(mut slot) = self.link_rewrite_hook.lock() {
            *slot = hook;
        }
    }

    /// Note that the Markdown file at `from` is now at `to`, so its links
    /// follow on the next [`Self::follow_moved_links`].
    pub(super) fn note_moved_links(&self, moves: impl IntoIterator<Item = (String, String)>) {
        if let Ok(mut pending) = self.moved_links.lock() {
            pending.extend(
                moves
                    .into_iter()
                    .filter(|(from, to)| from != to && is_markdown_path(to)),
            );
        }
    }

    /// Note that the folder at `from` is now at `to`, for each Markdown
    /// file in it.
    pub(super) fn note_moved_folder(&self, from: &str, to: &str) {
        let Ok(paths) = self
            .index
            .lock()
            .map_err(|_| lock_error("links_read"))
            .and_then(|index| index.markdown_paths())
        else {
            return;
        };
        let prefix = format!("{to}/");
        let moves = paths.into_iter().filter_map(|path| {
            let rest = path.strip_prefix(&prefix)?;
            Some((format!("{from}/{rest}"), path))
        });
        self.note_moved_links(moves.collect::<Vec<_>>());
    }

    /// Rewrite the links that led to each file moved since the last call,
    /// so they lead to it where it is now. A file that changed while Noura
    /// rewrote it keeps its bytes and joins the workspace's problems.
    /// Never fails the change that moved the files: a rewrite that can't
    /// finish is logged.
    pub(super) fn follow_moved_links(&self) {
        // The next change that ends without the lock follows them.
        if holds_write_lock() {
            return;
        }
        let moves = match self.moved_links.lock() {
            Ok(mut pending) => std::mem::take(&mut *pending),
            Err(_) => return,
        };
        if moves.is_empty() {
            return;
        }
        if let Err(error) = self.rewrite_links(&moves) {
            tracing::warn!(code = %error.code, "links to moved files weren't all updated");
        }
    }

    fn rewrite_links(&self, moves: &[(String, String)]) -> Result<()> {
        const OPERATION: &str = "links_rewrite";
        // A chain of moves, as `A` to `B` then `B` to `C`, counts once.
        let mut moved: BTreeMap<String, String> = BTreeMap::new();
        for (from, to) in moves {
            let earliest = moved
                .iter()
                .find(|(_, now)| *now == from)
                .map(|(first, _)| first.clone());
            match earliest {
                Some(first) => {
                    moved.insert(first, to.clone());
                }
                None => {
                    moved.insert(from.clone(), to.clone());
                }
            }
        }
        moved.retain(|from, to| from != to);
        if moved.is_empty() {
            return Ok(());
        }
        let (now_paths, sources) = {
            let index = self.index.lock().map_err(|_| lock_error(OPERATION))?;
            let names: Vec<String> = moved.keys().flat_map(|from| names_of(from)).collect();
            (index.markdown_paths()?, index.link_sources(&names)?)
        };
        let back: HashMap<&str, &str> = moved
            .iter()
            .map(|(from, to)| (to.as_str(), from.as_str()))
            .collect();
        let before = LinkTargets::new(
            now_paths
                .iter()
                .map(|path| back.get(path.as_str()).copied().unwrap_or(path)),
        );
        let after = LinkTargets::new(now_paths.iter().map(String::as_str));
        for (source, _, links) in sources {
            let was = back.get(source.as_str()).copied().unwrap_or(&source);
            let mut renames: HashMap<String, String> = HashMap::new();
            for target in links {
                let Some(old) = before.resolve(was, &target) else {
                    continue;
                };
                let Some(new) = moved.get(&old) else {
                    continue;
                };
                if after.resolve(&source, &target).as_deref() == Some(new) {
                    continue;
                }
                renames.insert(target, after.link_for(&source, new));
            }
            if renames.is_empty() {
                continue;
            }
            let moved_to = renames.values().next().cloned().unwrap_or_default();
            match self.rewrite_file_links(&source, &renames) {
                Ok(()) => {
                    if let Ok(mut stale) = self.stale_links.lock() {
                        stale.remove(&source);
                    }
                }
                Err(error) => {
                    tracing::warn!(code = %error.code, path = %source, "a file's links weren't updated");
                    if let Ok(mut stale) = self.stale_links.lock() {
                        let target = renames.keys().next().cloned().unwrap_or_default();
                        stale.insert(source.clone(), StaleLink { target, moved_to });
                    }
                }
            }
        }
        Ok(())
    }

    /// Give each link in the file at `source` whose target `renames` names
    /// its new target, writing the file only if it still holds the bytes
    /// read here.
    fn rewrite_file_links(&self, source: &str, renames: &HashMap<String, String>) -> Result<()> {
        const OPERATION: &str = "links_rewrite";
        let path = resolve_for_write(&self.root, source, OPERATION)?;
        let _guard = self.write_lock(OPERATION)?;
        self.collaboration_guard_file_mutation(source)?;
        let bytes = read_downloaded(&path, source, OPERATION)?;
        let read_revision = markdown::revision(&bytes);
        let next = match rewritten_links(source, &bytes, renames) {
            Ok(Some(next)) => next,
            Ok(None) => return Ok(()),
            Err(()) => {
                return Err(CoreError::validation(
                    "links_unwritable",
                    "The file's links can't be rewritten without changing other text",
                    OPERATION,
                ));
            }
        };
        #[cfg(any(test, feature = "test-hooks"))]
        if let Ok(hook) = self.link_rewrite_hook.lock()
            && let Some(hook) = hook.as_ref()
        {
            hook(source);
        }
        self.expect_own_write(source, Some(&markdown::revision(&next)));
        atomic_write_checked(
            &self.root,
            Path::new(source),
            &next,
            Some(&read_revision),
            OPERATION,
        )
        .inspect_err(|_| self.forget_own_write(source))?;
        if let Err(error) = self.reconcile_paths(&[source.to_owned()], EventSource::Application) {
            tracing::warn!(code = %error.code, "the index couldn't follow a link rewrite");
            self.dirty.store(true, Ordering::SeqCst);
        }
        self.emit(
            "file:changed",
            EventSource::Application,
            serde_json::json!({ "paths": [source] }),
        );
        Ok(())
    }

    /// The files whose links Noura couldn't update and that still hold
    /// them, as problems.
    pub(super) fn stale_link_diagnostics(&self) -> Vec<crate::Diagnostic> {
        let Ok(mut stale) = self.stale_links.lock() else {
            return Vec::new();
        };
        if stale.is_empty() {
            return Vec::new();
        }
        if let Ok(index) = self.index.lock() {
            let names: Vec<String> = stale
                .values()
                .map(|link| workspace_format::links::link_name(&link.target))
                .collect();
            if let Ok(sources) = index.link_sources(&names) {
                let held: HashSet<(String, String)> = sources
                    .into_iter()
                    .flat_map(|(path, _, targets)| {
                        targets
                            .into_iter()
                            .map(move |target| (path.clone(), target))
                    })
                    .collect();
                stale.retain(|path, link| held.contains(&(path.clone(), link.target.clone())));
            }
        }
        stale
            .iter()
            .map(|(path, link)| crate::Diagnostic {
                code: "stale_link".into(),
                message: format!(
                    "Links to {} weren't updated: the file changed, or the new link \
                     can't stand where the old one did",
                    link.moved_to
                ),
                relative_path: Some(path.clone()),
                object_id: None,
            })
            .collect()
    }
}

fn is_markdown_path(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("md"))
}

/// The bytes of the Markdown file at `source`, holding `bytes` now, with
/// the links `renames` names rewritten, or `None` when none changes.
/// Everything else keeps its bytes: the text changes only where a link's
/// target stands. When that would change a property in another way, as a
/// title with a quote in single-quoted YAML would, a managed object's
/// frontmatter is written again instead, and any other file, or one that
/// isn't UTF-8, is an error.
fn rewritten_links(
    source: &str,
    bytes: &[u8],
    renames: &HashMap<String, String>,
) -> std::result::Result<Option<Vec<u8>>, ()> {
    let rename = |target: &str| renames.get(target).cloned();
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let Some(next) = rewrite_body_links(text, rename) else {
        return Ok(None);
    };
    let parsed = markdown::parse_markdown(source, bytes);
    let expected = expected_after(&parsed, &rename);
    let reparsed = markdown::parse_markdown(source, next.as_bytes());
    if same_content(&expected, &reparsed) {
        return Ok(Some(next.into_bytes()));
    }
    match expected {
        ParsedMarkdown::Managed(object) => workspace_format::serialize_object_over(bytes, &object)
            .map(Some)
            .map_err(|_| ()),
        _ => Err(()),
    }
}

/// What `parsed` holds once each link `rename` renames takes its new
/// target.
fn expected_after(
    parsed: &ParsedMarkdown,
    rename: &impl Fn(&str) -> Option<String>,
) -> ParsedMarkdown {
    let body = |text: &str| rewrite_body_links(text, rename).unwrap_or_else(|| text.to_owned());
    let properties = |values: &BTreeMap<String, serde_json::Value>| {
        values
            .iter()
            .map(|(key, value)| {
                let next = rewrite_property_links(value, &mut |target| rename(target));
                (key.clone(), next.unwrap_or_else(|| value.clone()))
            })
            .collect::<BTreeMap<_, _>>()
    };
    match parsed {
        ParsedMarkdown::Managed(object) => {
            let mut object = object.clone();
            object.body = body(&object.body);
            object.properties = properties(&object.properties);
            ParsedMarkdown::Managed(object)
        }
        ParsedMarkdown::Unmanaged {
            title,
            body: text,
            frontmatter,
        } => ParsedMarkdown::Unmanaged {
            title: title.clone(),
            body: body(text),
            frontmatter: frontmatter.as_ref().map(properties),
        },
        ParsedMarkdown::Malformed {
            title,
            body: text,
            error,
        } => ParsedMarkdown::Malformed {
            title: title.clone(),
            body: body(text),
            error: error.clone(),
        },
    }
}

fn same_content(left: &ParsedMarkdown, right: &ParsedMarkdown) -> bool {
    match (left, right) {
        (ParsedMarkdown::Managed(left), ParsedMarkdown::Managed(right)) => {
            left.id == right.id
                && left.object_type == right.object_type
                && left.body == right.body
                && left.properties == right.properties
                && left.created == right.created
                && left.updated == right.updated
        }
        (
            ParsedMarkdown::Unmanaged {
                body: left_body,
                frontmatter: left_frontmatter,
                ..
            },
            ParsedMarkdown::Unmanaged {
                body: right_body,
                frontmatter: right_frontmatter,
                ..
            },
        ) => left_body == right_body && left_frontmatter == right_frontmatter,
        (
            ParsedMarkdown::Malformed {
                body: left_body, ..
            },
            ParsedMarkdown::Malformed {
                body: right_body, ..
            },
        ) => left_body == right_body,
        _ => false,
    }
}
