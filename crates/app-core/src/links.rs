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

use crate::pdf::{EmbedProblem, PdfPicture, draw_page_once, is_pdf, page_in_link};
use crate::remote_images::{RemoteImageState, RemoteImages};
use crate::watch::Snapshot;
use crate::{EMBED_WIDTH, FileTree};

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
    pdfs: Arc<PdfEmbeds>,
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
        // A note or PDF that left the workspace takes what its embeds
        // showed with it.
        self.embeds
            .notes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|path, _| index.paths.contains(path));
        self.pdfs
            .pages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|(path, _), _| index.paths.contains(path));
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
    /// How many files show each name in the file tree, by lowercase name.
    shown_names: HashMap<String, usize>,
    /// Each indexed file's revision.
    revisions: HashMap<String, String>,
}

impl LinkIndex {
    pub(crate) fn new<'a>(files: impl IntoIterator<Item = &'a str>) -> Self {
        let mut index = Self::default();
        for file in files {
            let name = file.rsplit('/').next().unwrap_or(file).to_lowercase();
            *index.shown_names.entry(shown_lowercase(file)).or_default() += 1;
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

/// What the PDF embeds drawn so far show, kept across listings.
#[derive(Default)]
struct PdfEmbeds {
    /// Each embed by the PDF's path and the page its link names.
    pages: Mutex<HashMap<(String, Option<u32>), PdfEmbedDraw>>,
    /// Counts the drawings finished so far.
    drawn: AtomicU64,
}

/// What one PDF embed shows, and which version of the file it drew.
struct PdfEmbedDraw {
    /// The listing the newest check or drawing started under.
    listing: u64,
    /// The size and modification time of the file the drawing came from.
    stamp: Option<(u64, Option<std::time::SystemTime>)>,
    /// `None` until the first drawing ends.
    shown: Option<PdfEmbedState>,
    /// A check or drawing runs on another thread.
    busy: bool,
}

/// What an embed of a PDF in a note shows.
#[derive(Debug, Clone, PartialEq)]
pub enum PdfEmbedState {
    /// The page draws on another thread.
    Drawing,
    /// The page the embed's link names, drawn [`EMBED_WIDTH`] pixels wide.
    ///
    /// [`EMBED_WIDTH`]: crate::EMBED_WIDTH
    Drawn(PdfPicture),
    /// The PDF shows no page, for this reason.
    Problem(EmbedProblem),
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

/// How many files typing `[[` suggests at most.
pub const LINK_SUGGESTION_LIMIT: usize = 10;

/// A workspace file that typing `[[` in a note offers to link to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSuggestion {
    /// The name the file tree shows: Markdown files appear without `.md`.
    pub name: String,
    /// The folder that holds the file, `""` at the top of the workspace,
    /// when another file shows the same name.
    pub folder: Option<String>,
    /// What picking the file puts between `[[` and `]]`: its name, or for
    /// a name another file shares, its path. Neither keeps `.md`.
    pub link: String,
}

/// The name the file tree shows for the file at `path`, in lowercase.
fn shown_lowercase(path: &str) -> String {
    embed_title(path).to_lowercase()
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

    /// Changes whenever a PDF embed's page finishes drawing.
    pub fn pdf_embeds_seq(&self) -> u64 {
        self.files.pdfs.drawn.load(Ordering::SeqCst)
    }

    /// The workspace PDF that `target` leads to, and the page its
    /// `#page=` fragment names: `None` for none, or for one that isn't a
    /// whole number from 1. `None` altogether when `target` names no PDF
    /// in the workspace.
    pub fn pdf_link(&self, target: &str) -> Option<(String, Option<u32>)> {
        let path = self.resolve(target)?;
        is_pdf(&path).then(|| (path, page_in_link(target)))
    }

    /// What an `![[file.pdf#page=2]]` embed shows: the page its fragment
    /// names, page 1 for none or an invalid one, or the last page for one
    /// past the end. `None` when `target` names no PDF in the workspace.
    /// The first ask starts drawing the page on another thread and answers
    /// [`Drawing`](PdfEmbedState::Drawing) until it ends; `drawn` runs when
    /// it ends. After a newer listing of the files, the next ask checks the
    /// file and draws it again if another app replaced it, and answers
    /// with the last drawing meanwhile. Embedding writes nothing.
    pub fn pdf_embed(
        &self,
        target: &str,
        drawn: impl FnOnce() + Send + 'static,
    ) -> Option<PdfEmbedState> {
        let listing = self.files.get().seq;
        let (path, asked) = self.pdf_link(target)?;
        let pdfs = Arc::clone(&self.files.pdfs);
        let mut pages = pdfs
            .pages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (path.clone(), asked);
        let embed = pages.entry(key.clone()).or_insert_with(|| PdfEmbedDraw {
            listing,
            stamp: None,
            shown: None,
            busy: false,
        });
        if (embed.shown.is_none() || embed.listing != listing) && !embed.busy {
            embed.listing = listing;
            embed.busy = true;
            let known = embed.shown.is_some().then_some(embed.stamp);
            let engine = Arc::clone(&self.engine);
            let pdfs = Arc::clone(&pdfs);
            std::thread::spawn(move || {
                // The version comes first, so a write during the drawing
                // makes the next check differ.
                let stamp = engine.file_version(&path).ok();
                let shown = (known != Some(stamp)).then(|| {
                    match draw_page_once(&engine, &path, asked, EMBED_WIDTH) {
                        Ok(picture) => PdfEmbedState::Drawn(picture),
                        Err(problem) => PdfEmbedState::Problem(problem),
                    }
                });
                if let Some(embed) = pdfs
                    .pages
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get_mut(&key)
                {
                    embed.busy = false;
                    if let Some(shown) = shown {
                        embed.stamp = stamp;
                        embed.shown = Some(shown);
                    }
                }
                pdfs.drawn.fetch_add(1, Ordering::SeqCst);
                drawn();
            });
        }
        Some(embed.shown.clone().unwrap_or(PdfEmbedState::Drawing))
    }

    /// The workspace file `target` leads to, or `None` when it names no
    /// file in the workspace. A `#heading` part is ignored.
    pub fn resolve(&self, target: &str) -> Option<String> {
        resolve_link_path(&self.source, target, &self.files.get())
    }

    /// Whether following a link to `target` from this note leads
    /// somewhere: to a web page for an `http`, `https` or `www.` address,
    /// as [`App::follow_note_link`](crate::App::follow_note_link) sends
    /// it, or to a workspace file.
    pub fn leads_somewhere(&self, target: &str) -> bool {
        web_address(target.trim()).is_some() || self.resolve(target).is_some()
    }

    /// The workspace files that typing `[[typed` suggests, best first, as
    /// quick open ranks them: names before paths, an exact name before a
    /// name that starts with what you typed. Nothing typed lists files by
    /// name. At most [`LINK_SUGGESTION_LIMIT`].
    pub fn suggest(&self, typed: &str) -> Vec<LinkSuggestion> {
        let index = self.files.get();
        let paths: Vec<String> = if typed.trim().is_empty() {
            let mut paths: Vec<(String, &String)> = index
                .paths
                .iter()
                .map(|path| (crate::files::sort_key(&embed_title(path)), path))
                .collect();
            paths.sort();
            paths.into_iter().map(|(_, path)| path.clone()).collect()
        } else {
            crate::quick_open::rank(index.paths.iter().map(String::as_str), typed)
                .into_iter()
                .map(|found| found.relative_path)
                .collect()
        };
        paths
            .into_iter()
            .take(LINK_SUGGESTION_LIMIT)
            .map(|path| self.suggestion(&index, &path))
            .collect()
    }

    /// How `[[` offers the file at `path`. A unique name links by name and
    /// a shared one by path. When that leads to another file from this
    /// note, the link is the path from the top of the workspace, then the
    /// path with `.md`, as for `Report.pdf.md` beside `Report.pdf`.
    fn suggestion(&self, index: &LinkIndex, path: &str) -> LinkSuggestion {
        let name = embed_title(path);
        let shared = index
            .shown_names
            .get(&name.to_lowercase())
            .is_some_and(|count| *count > 1);
        let without_md = if crate::notes::is_markdown(path) {
            &path[..path.len() - 3]
        } else {
            path
        };
        let mut links = vec![
            without_md.to_owned(),
            format!("/{without_md}"),
            path.to_owned(),
            format!("/{path}"),
        ];
        if !shared {
            links.insert(0, name.clone());
        }
        let leads_here =
            |link: &String| resolve_link_path(&self.source, link, index).as_deref() == Some(path);
        // A file no link reaches, as `Plan` beside `Plan.md`, keeps the
        // first.
        let link = links
            .iter()
            .find(|link| leads_here(link))
            .unwrap_or(&links[0])
            .clone();
        LinkSuggestion {
            folder: shared.then(|| folder_of(path).to_owned()),
            name,
            link,
        }
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
pub(crate) fn link_path(target: &str) -> String {
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

/// The web page a link to `target` opens: `target` itself when it starts
/// with `http://` or `https://`, and `https://` before it when it starts
/// with `www.`.
pub(crate) fn web_address(target: &str) -> Option<String> {
    let starts_with = |prefix: &str| {
        target
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    };
    if starts_with("http://") || starts_with("https://") {
        Some(target.to_owned())
    } else if starts_with("www.") {
        Some(format!("https://{target}"))
    } else {
        None
    }
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
