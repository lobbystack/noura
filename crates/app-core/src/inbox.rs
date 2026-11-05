//! The Inbox: one place to start the day. It greets you, lists workspace
//! problems you can fix yourself, holds the sections enabled plugins add,
//! and shows the notes you changed last.

use std::sync::Arc;

use plugin_host::{Node, NodeKind, View};

use crate::{
    AppError, EntryKind, Page, Platform, Problem, files::shown_name, plugins::PluginSection,
};

/// How many notes Recent lists.
pub const RECENT_NOTES: usize = 5;

/// What the Inbox page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbox {
    /// Today's greeting for the time of day.
    pub greeting: &'static str,
    /// Workspace problems you can fix yourself, each with links to the
    /// files it names.
    pub needs_attention: Vec<Attention>,
    /// The enabled plugins' sections, in their plugins' rail order.
    pub sections: Vec<InboxSection>,
    /// The notes whose files changed last, newest first.
    pub recent: Vec<RecentNote>,
}

/// The greeting at `hour` o'clock: Working late before 5 am, Good
/// morning until noon, Good afternoon until 6 pm, then Good evening.
pub(crate) fn greeting(hour: u8) -> &'static str {
    match hour {
        0..5 => "Working late",
        5..12 => "Good morning",
        12..18 => "Good afternoon",
        _ => "Good evening",
    }
}

/// What fills an Inbox section that lines of text fill, such as one a
/// client adds with [`App::add_inbox_section`](crate::App::add_inbox_section).
/// Running plugins fill theirs with views they draw, through the
/// `inbox-sections` capability.
pub trait InboxSectionProvider: Send + Sync {
    /// The section's lines, top to bottom, or why they couldn't load.
    fn load(&self) -> Result<Vec<String>, AppError>;
}

impl<F> InboxSectionProvider for F
where
    F: Fn() -> Result<Vec<String>, AppError> + Send + Sync,
{
    fn load(&self) -> Result<Vec<String>, AppError> {
        self()
    }
}

/// A plugin's section in the Inbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxSection {
    /// The plugin the section belongs to.
    pub plugin_id: String,
    /// The section's ID within its plugin, which a press in it names.
    pub section_id: String,
    /// The section's heading, such as "Due today".
    pub title: String,
    /// What the section shows, or why it couldn't load or draw, in words.
    pub content: Result<View, String>,
}

/// A section filled by a provider and what it holds, while its plugin is
/// on.
#[derive(Clone)]
pub(crate) struct Slot {
    plugin_id: String,
    /// The plugin's rail page, whose place the section takes.
    page_id: Option<String>,
    title: String,
    provider: Arc<dyn InboxSectionProvider>,
    content: Result<Vec<String>, String>,
}

/// A view of one text node per line.
fn lines_view(lines: &[String]) -> Result<View, String> {
    let mut nodes = vec![Node {
        id: "section".to_owned(),
        kind: NodeKind::Panel(None),
        children: Vec::new(),
    }];
    for (index, line) in lines.iter().enumerate() {
        nodes[0]
            .children
            .push(u32::try_from(index + 1).unwrap_or(u32::MAX));
        nodes.push(Node {
            id: format!("line-{index}"),
            kind: NodeKind::Text(line.clone()),
            children: Vec::new(),
        });
    }
    View::from_nodes(nodes).map_err(|error| error.to_string())
}

/// The sections enabled plugins add, in the order they arrived.
#[derive(Clone, Default)]
pub(crate) struct Sections(Vec<Slot>);

impl Sections {
    /// Add the section of the plugin `plugin_id`, or replace the one it
    /// has, and load it.
    pub(crate) fn add(
        &mut self,
        plugin_id: &str,
        page_id: Option<&str>,
        title: &str,
        provider: Arc<dyn InboxSectionProvider>,
    ) {
        let slot = Slot {
            plugin_id: plugin_id.to_owned(),
            page_id: page_id.map(str::to_owned),
            title: title.to_owned(),
            content: load(provider.as_ref()),
            provider,
        };
        match self.0.iter_mut().find(|slot| slot.plugin_id == plugin_id) {
            Some(shown) => *shown = slot,
            None => self.0.push(slot),
        }
    }

    /// Take out the section of the plugin `plugin_id`. Returns whether it
    /// was there.
    pub(crate) fn remove(&mut self, plugin_id: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|slot| slot.plugin_id != plugin_id);
        self.0.len() != before
    }

    /// Load every section again. Returns whether any shows something new.
    pub(crate) fn reload(&mut self) -> bool {
        let mut changed = false;
        for slot in &mut self.0 {
            let content = load(slot.provider.as_ref());
            if content != slot.content {
                slot.content = content;
                changed = true;
            }
        }
        changed
    }

    /// These sections and the running plugins' `plugins`, in their
    /// plugins' rail order, given as `rail`. A section whose plugin has no
    /// page in the rail comes after the others: these in the order they
    /// arrived, then the plugins' by plugin.
    pub(crate) fn ordered(&self, rail: &[&Page], plugins: Vec<PluginSection>) -> Vec<InboxSection> {
        let place = |page_id: Option<&str>| {
            page_id
                .and_then(|page_id| rail.iter().position(|page| page.id == page_id))
                .unwrap_or(usize::MAX)
        };
        let mut sections: Vec<(usize, InboxSection)> = self
            .0
            .iter()
            .map(|slot| {
                let section = InboxSection {
                    plugin_id: slot.plugin_id.clone(),
                    section_id: slot.plugin_id.clone(),
                    title: slot.title.clone(),
                    content: slot.content.clone().and_then(|lines| lines_view(&lines)),
                };
                (place(slot.page_id.as_deref()), section)
            })
            .collect();
        sections.extend(plugins.into_iter().map(|section| {
            let place = place(section.page_id.as_deref());
            let section = InboxSection {
                plugin_id: section.plugin_id,
                section_id: section.section_id,
                title: section.title,
                content: section.content,
            };
            (place, section)
        }));
        sections.sort_by_key(|(place, _)| *place);
        sections.into_iter().map(|(_, section)| section).collect()
    }
}

/// Load a section. A failure stays in the section, in words; the details
/// go to the log.
fn load(provider: &dyn InboxSectionProvider) -> Result<Vec<String>, String> {
    provider.load().map_err(|error| {
        tracing::warn!(code = %error.code, "an Inbox section couldn't load");
        error.message
    })
}

/// A workspace problem in Needs attention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attention {
    /// The problem in plain words.
    pub message: String,
    /// A link to each file or folder the problem names.
    pub links: Vec<FileLink>,
}

/// A link to a file or folder in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLink {
    /// What the link says, naming its file or folder.
    pub label: String,
    pub relative_path: String,
    pub action: LinkAction,
}

/// What following a [`FileLink`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAction {
    /// Open the file in a tab.
    Open,
    /// Show the file or folder in the file manager, for problems you fix
    /// in another app, such as a name Noura can't represent.
    Reveal,
}

impl Attention {
    pub(crate) fn of(problem: &Problem, platform: Platform) -> Self {
        let action = match problem.code.as_str() {
            "unsupported_path" | "unreadable_file" | "unreadable_folder" => LinkAction::Reveal,
            _ => LinkAction::Open,
        };
        let links = problem
            .paths
            .iter()
            .map(|path| FileLink {
                label: match action {
                    LinkAction::Open => format!("Open {path}"),
                    LinkAction::Reveal => reveal_label(path, platform),
                },
                relative_path: path.clone(),
                action,
            })
            .collect();
        Self {
            message: problem.message.clone(),
            links,
        }
    }
}

/// "Show Notes in Finder", in the words of the platform's file manager.
fn reveal_label(path: &str, platform: Platform) -> String {
    let manager = match platform {
        Platform::MacOs => "Finder",
        Platform::Windows => "Explorer",
        Platform::Linux => "the file manager",
    };
    format!("Show {path} in {manager}")
}

/// A note in Recent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentNote {
    /// The note's title: its file name without the extension.
    pub title: String,
    pub relative_path: String,
    /// Whether another note in Recent has the same title, such as
    /// `Plan.md` and `Ideas/Plan.md`, so the note shows its path to tell
    /// them apart.
    pub shares_title: bool,
}

impl RecentNote {
    /// The notes at `paths`, in their order, each marked when its title
    /// isn't unique among them.
    pub(crate) fn list(paths: &[String]) -> Vec<Self> {
        let titles: Vec<String> = paths
            .iter()
            .map(|path| {
                let name = path.rsplit('/').next().unwrap_or(path);
                shown_name(name, EntryKind::File)
            })
            .collect();
        paths
            .iter()
            .zip(&titles)
            .map(|(path, title)| Self {
                title: title.clone(),
                relative_path: path.clone(),
                shares_title: titles.iter().filter(|other| *other == title).count() > 1,
            })
            .collect()
    }
}
