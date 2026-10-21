//! Where the links and embeds in a note lead. A target resolves the way
//! Obsidian and today's app resolve it: relative to the note, then from the
//! workspace root, then by file name anywhere in the workspace, ignoring
//! case and preferring the note's folder and then the shortest path. `Plan`
//! finds `Plan.md`. Resolution reads the newest listing of the workspace's
//! files, which holds nothing outside the workspace, so a link never leads
//! out of it.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use local_core::WorkspaceEngine;

use crate::FileTree;
use crate::remote_images::{RemoteImageState, RemoteImages};
use crate::watch::Snapshot;

/// How much of an embedded note's text an embed shows, in characters.
pub const EMBED_PREVIEW_CHARS: usize = 800;

/// How much of an embedded note an embed reads, in bytes: enough for
/// frontmatter as long as the editor's 256 KiB lookahead and the text
/// after it.
const EMBED_READ_BYTES: usize = 256 * 1024 + 4 * EMBED_PREVIEW_CHARS;

/// The open workspace's files as its newest listing shows them, and what
/// the notes embedded so far show. The session updates it with each
/// listing, and every note's links read it.
#[derive(Clone, Default)]
pub(crate) struct LinkFiles {
    index: Arc<RwLock<Arc<LinkIndex>>>,
    embeds: Arc<Embeds>,
}

impl LinkFiles {
    /// Take the files `tree` lists, whose revisions `snapshot` holds.
    pub(crate) fn list(&self, tree: &FileTree, snapshot: &Snapshot) {
        let mut current = self
            .index
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut index = LinkIndex::new(tree.file_paths());
        index.seq = current.seq + 1;
        index.revisions = snapshot
            .entries
            .iter()
            .filter_map(|entry| {
                let revision = entry.revision.clone()?;
                Some((entry.relative_path.clone(), revision))
            })
            .collect();
        // A note that left the workspace takes what its embeds showed
        // with it.
        self.embeds
            .notes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|path, _| index.paths.contains(path));
        *current = Arc::new(index);
    }

    fn get(&self) -> Arc<LinkIndex> {
        self.index
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// The workspace's files, by path and by lowercase file name, so a note
/// with many links resolves them quickly in a large workspace.
#[derive(Default)]
pub(crate) struct LinkIndex {
    /// Counts the listings so far.
    seq: u64,
    paths: HashSet<String>,
    by_name: HashMap<String, Vec<String>>,
    /// Each indexed file's revision.
    revisions: HashMap<String, String>,
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

    /// Which version of the file at `path` this listing shows: its
    /// revision, or for a file without one, the listing itself.
    fn version(&self, path: &str) -> Version {
        match self.revisions.get(path) {
            Some(revision) => Version::Revision(revision.clone()),
            None => Version::Listing(self.seq),
        }
    }
}

/// A version of a file, as a listing shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Version {
    Revision(String),
    /// A file without a revision counts as changed in every listing.
    Listing(u64),
}

/// What the notes embedded so far show, kept across listings.
#[derive(Default)]
struct Embeds {
    notes: Mutex<HashMap<String, EmbedRead>>,
    /// Counts the reads finished so far.
    read: AtomicU64,
}

/// What an embed of one note shows, and which version of the note it read.
struct EmbedRead {
    /// The version the newest read started on.
    version: Version,
    /// `None` until the first read ends, then what the newest read found:
    /// `None` inside when the note couldn't be read.
    shown: Option<Option<NoteEmbed>>,
    /// A read runs on another thread.
    reading: bool,
}

/// The largest workspace image a note draws, in bytes, as today's app.
pub const MAX_IMAGE_BYTES: i64 = 20 * 1024 * 1024;

/// The image formats a note draws inline, as today's app shows them.
pub const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif"];

/// Whether `path` names an image in a format a note draws.
pub fn is_image(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && IMAGE_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
    })
}

/// A workspace image a note draws: its path, its bytes and their version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteImage {
    pub path: String,
    pub bytes: Vec<u8>,
    pub version: ImageVersion,
}

/// Which file an image link led to and which version of it: its path, its
/// size and its modification time. A note reads an image again only when
/// this changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageVersion {
    path: String,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

/// The path from the folder of the note at `source` to the workspace
/// path `path`, for reads that resolve relative to the note.
fn relative_target(source: &str, path: &str) -> String {
    let from: Vec<&str> = folder_of(source)
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let to: Vec<&str> = path.split('/').collect();
    let mut shared = 0;
    while shared < from.len() && shared + 1 < to.len() && from[shared] == to[shared] {
        shared += 1;
    }
    std::iter::repeat_n("..", from.len() - shared)
        .chain(to[shared..].iter().copied())
        .collect::<Vec<_>>()
        .join("/")
}

/// What an embedded note shows: its title, which is its file name, and
/// the first [`EMBED_PREVIEW_CHARS`] characters of its text. A note of
/// Noura's own shows the text after its frontmatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEmbed {
    pub title: String,
    pub text: String,
}

/// Where an embedded note stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteEmbedState {
    /// Its read runs on another thread. The title comes from the file name
    /// and is already known.
    Reading {
        title: String,
    },
    Read(NoteEmbed),
}

/// The title an embed of the note at `path` shows: its file name.
fn embed_title(path: &str) -> String {
    crate::files::shown_name(
        path.rsplit('/').next().unwrap_or(path),
        crate::EntryKind::File,
    )
}

/// What the links in one note lead to. A note's editor holds one and asks
/// it about each link and embed it draws; it reads the workspace's newest
/// listing, so links follow files as they appear, move and go.
#[derive(Clone)]
pub struct NoteLinks {
    source: String,
    files: LinkFiles,
    engine: Arc<WorkspaceEngine>,
    remote: Arc<RemoteImages>,
}

impl NoteLinks {
    pub(crate) fn new(
        source: &str,
        files: LinkFiles,
        engine: Arc<WorkspaceEngine>,
        remote: Arc<RemoteImages>,
    ) -> Self {
        Self {
            source: source.to_owned(),
            files,
            engine,
            remote,
        }
    }

    /// Where the remote image at `url` stands. Asking sends no request.
    pub fn remote_image(&self, url: &str) -> RemoteImageState {
        self.remote.state(url)
    }

    /// Load the remote image at `url`, as Load image asks: once, unless it
    /// failed, when this tries again. `done` runs when the load ends.
    pub fn load_remote_image(&self, url: &str, done: impl FnOnce() + Send + 'static) {
        self.remote.load(url, done);
    }

    /// Changes whenever a remote image's load ends.
    pub fn remote_seq(&self) -> u64 {
        self.remote.seq()
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

    /// Changes whenever an embedded note's read finishes.
    pub fn embeds_seq(&self) -> u64 {
        self.files.embeds.read.load(Ordering::SeqCst)
    }

    /// The workspace file `target` leads to, or `None` when it names no
    /// file in the workspace. A `#heading` part is ignored.
    pub fn resolve(&self, target: &str) -> Option<String> {
        resolve_link_path(&self.source, target, &self.files.get())
    }

    /// What an `![[target]]` embed shows when `target` names a Markdown
    /// note, or `None` when it names anything else or nothing, or the note
    /// can't be read. The first ask starts a read of the note's start on
    /// another thread and answers [`Reading`](NoteEmbedState::Reading)
    /// until it ends; `read` runs when that read ends. Once the note's
    /// revision changes in a newer listing, the next ask reads it again and
    /// answers what the last read found until the new one ends. Embedding
    /// writes nothing.
    pub fn embed(
        &self,
        target: &str,
        read: impl FnOnce() + Send + 'static,
    ) -> Option<NoteEmbedState> {
        let index = self.files.get();
        let path = resolve_link_path(&self.source, target, &index)?;
        if !crate::notes::is_markdown(&path) {
            return None;
        }
        let version = index.version(&path);
        let embeds = Arc::clone(&self.files.embeds);
        let mut notes = embeds
            .notes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let note = notes.entry(path.clone()).or_insert_with(|| EmbedRead {
            version: version.clone(),
            shown: None,
            reading: false,
        });
        let unread = note.shown.is_none() && !note.reading;
        if unread || (note.version != version && !note.reading) {
            note.version = version;
            note.reading = true;
            let links = self.clone();
            let embeds = Arc::clone(&embeds);
            let path = path.clone();
            std::thread::spawn(move || {
                let embed = links.read_embed(&path);
                if let Some(note) = embeds
                    .notes
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get_mut(&path)
                {
                    note.shown = Some(embed);
                    note.reading = false;
                }
                embeds.read.fetch_add(1, Ordering::SeqCst);
                read();
            });
        }
        match &note.shown {
            Some(shown) => shown.clone().map(NoteEmbedState::Read),
            None => Some(NoteEmbedState::Reading {
                title: embed_title(&path),
            }),
        }
    }

    /// Which version of the workspace image `target` names is on disk,
    /// without reading it: `None` when it names no image in a format Noura
    /// shows, or one Noura may not read, such as a file behind a symlink.
    pub fn image_version(&self, target: &str) -> Option<ImageVersion> {
        let path = self.resolve(target)?;
        if !is_image(&path) {
            return None;
        }
        let relative = relative_target(&self.source, &path);
        let (found, len, modified) = self
            .engine
            .local_asset_version(&self.source, &relative)
            .ok()?;
        (found == path).then_some(ImageVersion {
            path,
            len,
            modified,
        })
    }

    /// The workspace image `target` names, read through the workspace's
    /// containment checks: `None` when it names no image in a format
    /// Noura shows, sits behind a symlink, can't be read or is larger than
    /// [`MAX_IMAGE_BYTES`].
    pub fn image(&self, target: &str) -> Option<NoteImage> {
        // The version comes first, so a write during the read makes the
        // next version differ and the note read the image again.
        let version = self.image_version(target)?;
        let relative = relative_target(&self.source, &version.path);
        let (read, bytes) = self
            .engine
            .read_local_asset(&self.source, &relative, MAX_IMAGE_BYTES)
            .ok()?;
        (read == version.path).then_some(NoteImage {
            path: read,
            bytes,
            version,
        })
    }

    fn read_embed(&self, path: &str) -> Option<NoteEmbed> {
        let body = self
            .engine
            .read_raw_markdown_start(path, EMBED_READ_BYTES)
            .ok()?;
        let text = match local_core::parse_markdown(path, body.as_bytes()) {
            local_core::ParsedMarkdown::Managed(_) => {
                let start = local_core::frontmatter_end(&body).unwrap_or(0);
                &body[start..]
            }
            _ => body.as_str(),
        };
        Some(NoteEmbed {
            title: embed_title(path),
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

/// Whether a link to `url` leads out of the workspace: a web address such
/// as `www.example.com`, or any address with a scheme, as `https:` or
/// `mailto:` have. A one-letter scheme is a Windows drive, as in `C:`.
pub fn is_external_link(url: &str) -> bool {
    let url = url.trim_start();
    url.get(..4)
        .is_some_and(|start| start.eq_ignore_ascii_case("www."))
        || (has_scheme(url) && url.find(':').is_some_and(|colon| colon > 1))
}

/// Whether `url` names a remote image, which loads only when you ask.
pub fn is_remote_image(url: &str) -> bool {
    let url = url.trim_start();
    ["http://", "https://"].iter().any(|scheme| {
        url.get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    })
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
    use super::{has_extension, is_external_link, is_remote_image, join_path, link_path};

    #[test]
    fn web_addresses_and_other_schemes_lead_out_of_the_workspace() {
        for url in [
            "https://example.com",
            "HTTP://example.com/a.png",
            "mailto:me@example.com",
            "obsidian://open?vault=x",
            "  www.example.com",
            "WWW.example.com/page",
        ] {
            assert!(is_external_link(url), "{url:?}");
        }
        for url in [
            "Plan",
            "notes/Plan.md",
            "C:/Users/me/Plan.md",
            "../a b.md",
            ":x",
        ] {
            assert!(!is_external_link(url), "{url:?}");
        }
    }

    #[test]
    fn only_http_and_https_images_are_remote() {
        assert!(is_remote_image("https://example.com/a.png"));
        assert!(is_remote_image(" HTTP://example.com/a.png"));
        assert!(!is_remote_image("pics/a.png"));
        assert!(!is_remote_image("ftp://example.com/a.png"));
        assert!(!is_remote_image("www.example.com/a.png"));
    }

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
