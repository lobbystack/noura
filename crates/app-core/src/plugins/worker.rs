//! The thread each running plugin's calls run on, so a plugin that loops,
//! allocates or crashes never holds up a window. The service sends it
//! jobs; it calls the plugin, draws the plugin's views and sends back what
//! changed.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender},
    },
    time::Duration,
};

use plugin_host::{
    Command, Event, Health, InboxSection, Page, PluginError, RunningPlugin, Surface, View,
    VirtualRow,
};

use super::{PluginView, Wake, navigation::Navigator};
use crate::session::WindowKey;

/// Something for a running plugin to do.
pub(super) enum Job {
    /// Ask what the plugin registers under each capability, then draw
    /// its views.
    Refresh,
    /// Tell the plugin about `event` at `surface`, which you did in the
    /// window given, then draw its views.
    Event(Surface, Event, Option<WindowKey>),
    /// Run the command palette's command `id`, which you ran in the window
    /// given, then draw its views.
    Command(String, Option<WindowKey>),
    /// Draw the plugin's views again.
    Redraw,
    /// Draw the properties panel of the object with this ID, whose file
    /// has this revision.
    Panel(String, String),
    /// Tell the plugin about `event` in the properties panel of the object
    /// with this ID, which you did in the window given, then draw that
    /// panel and its views again.
    PanelEvent(String, Event, Option<WindowKey>),
    /// Fetch `count` rows from `start` of the virtual list `node` on the
    /// page `page`, for the views drawn `drawn` times.
    Rows {
        page: String,
        node: String,
        start: u32,
        count: u32,
        drawn: u64,
    },
}

/// Rows the plugin returned for a virtual list, or why it couldn't.
pub(super) struct RowsFetched {
    pub(super) page: String,
    pub(super) node: String,
    pub(super) start: u32,
    pub(super) count: u32,
    pub(super) drawn: u64,
    pub(super) rows: Result<Vec<VirtualRow>, String>,
}

/// A properties panel the plugin drew.
pub(super) struct PanelDrawn {
    pub(super) object: String,
    /// The revision of the object's file it was asked to draw, or none
    /// when it drew the panel right after a press there.
    pub(super) revision: Option<String>,
    pub(super) view: PluginView,
}

/// A tool a plugin offers, with its schema compiled.
#[derive(Clone)]
pub(super) struct ToolEntry {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) input_schema: String,
    pub(super) validator: Arc<jsonschema::Validator>,
}

/// What a plugin registered under its registration capabilities.
#[derive(Clone, Default)]
pub(super) struct Registrations {
    pub(super) pages: Vec<Page>,
    /// The commands it adds to the command palette.
    pub(super) commands: Vec<Command>,
    pub(super) sections: Vec<InboxSection>,
    /// The title of its page in Settings.
    pub(super) settings: Option<String>,
    pub(super) tools: Vec<ToolEntry>,
}

impl Registrations {
    /// Every place the plugin draws a view.
    pub(super) fn surfaces(&self) -> Vec<Surface> {
        let pages = self.pages.iter().flat_map(|page| {
            [
                Surface::Page(page.id.clone()),
                Surface::Sidebar(page.id.clone()),
            ]
        });
        let sections = self
            .sections
            .iter()
            .map(|section| Surface::InboxSection(section.id.clone()));
        let settings = self.settings.iter().map(|_| Surface::Settings);
        pages.chain(sections).chain(settings).collect()
    }

    /// Keep only what the plugin may still register, after you revoked a
    /// capability.
    pub(super) fn keep(&mut self, holds: impl Fn(&str) -> bool) {
        if !holds("views") {
            self.pages.clear();
        }
        if !holds("commands") {
            self.commands.clear();
        }
        if !holds("inbox-sections") {
            self.sections.clear();
        }
        if !holds("settings") {
            self.settings = None;
        }
        if !holds("tools") {
            self.tools.clear();
        }
    }
}

/// What a job changed, for the service to show.
pub(super) struct Outcome {
    pub(super) id: String,
    /// Which start of the plugin ran the job. The service ignores an
    /// outcome from a start it replaced.
    pub(super) generation: u64,
    /// New registrations, after [`Job::Refresh`].
    pub(super) registrations: Option<Registrations>,
    /// Why Noura didn't add some of the object types the plugin
    /// registered, after [`Job::Refresh`].
    pub(super) refused_types: Option<Vec<String>>,
    /// Every view the plugin draws, as drawn now, or none when the job
    /// only drew a panel.
    pub(super) views: Option<BTreeMap<Surface, PluginView>>,
    /// The properties panel the job drew.
    pub(super) panel: Option<PanelDrawn>,
    /// The rows of a virtual list the job fetched.
    pub(super) rows: Option<RowsFetched>,
    /// The plugin ran code that may have written files: it handled an
    /// event or ran a command.
    pub(super) acted: bool,
    pub(super) health: Health,
    /// Why the job's event or command failed: the view the event happened
    /// in, or none for a command, and the reason in words.
    pub(super) failed: Option<(Option<Surface>, String)>,
}

/// What a plugin was doing when it failed.
#[derive(Clone, Copy)]
pub(super) enum Doing {
    Drawing,
    Handling,
    Running,
    Listing,
}

/// The worker's half of a running plugin.
pub(super) struct Worker {
    pub(super) id: String,
    pub(super) generation: u64,
    pub(super) plugin: Arc<RunningPlugin>,
    /// What the plugin asks windows to show, which only calls that answer
    /// what you did may use.
    pub(super) navigator: Arc<Navigator>,
    pub(super) jobs: Receiver<Job>,
    pub(super) outcomes: Sender<Outcome>,
    pub(super) wake: Option<Wake>,
    /// Set while a [`Job::Redraw`] waits, so the service queues no more.
    pub(super) redraw_queued: Arc<AtomicBool>,
}

impl Worker {
    /// Take jobs until the service drops the plugin.
    pub(super) fn run(self) {
        let mut registrations = Registrations::default();
        while let Ok(job) = self.jobs.recv() {
            let mut registered = None;
            let mut refused_types = None;
            let mut failed = None;
            let mut panel = None;
            let mut rows = None;
            let mut acted = true;
            let mut draws = true;
            match job {
                Job::Refresh => {
                    acted = false;
                    refused_types = Some(register_object_types(&self.plugin));
                    registrations = register(&self.plugin);
                    registered = Some(registrations.clone());
                }
                Job::Panel(object, revision) => {
                    acted = false;
                    draws = false;
                    panel = Some(PanelDrawn {
                        view: draw_panel(&self.plugin, &object),
                        object,
                        revision: Some(revision),
                    });
                }
                Job::Rows {
                    page,
                    node,
                    start,
                    count,
                    drawn,
                } => {
                    acted = false;
                    draws = false;
                    let fetched = self
                        .plugin
                        .rows(&page, &node, start, count)
                        .map_err(|error| {
                            tracing::warn!(%error, "a plugin failed to list rows");
                            failure_message(&error, Doing::Listing)
                        });
                    rows = Some(RowsFetched {
                        page,
                        node,
                        start,
                        count,
                        drawn,
                        rows: fetched,
                    });
                }
                Job::PanelEvent(object, event, origin) => {
                    self.navigator.set_origin(origin);
                    let handled = self.plugin.handle_panel_event(&object, &event);
                    self.navigator.set_origin(None);
                    let view = match handled {
                        Ok(()) => draw_panel(&self.plugin, &object),
                        Err(error) => {
                            tracing::warn!(%error, "a plugin failed to handle a press in a panel");
                            Err(failure_message(&error, Doing::Handling))
                        }
                    };
                    panel = Some(PanelDrawn {
                        object,
                        revision: None,
                        view,
                    });
                }
                Job::Event(surface, event, origin) => {
                    self.navigator.set_origin(origin);
                    let handled = self.plugin.handle_surface_event(&surface, &event);
                    self.navigator.set_origin(None);
                    if let Err(error) = handled {
                        tracing::warn!(%error, "a plugin failed to handle an event");
                        failed = Some((Some(surface), failure_message(&error, Doing::Handling)));
                    }
                }
                Job::Command(id, origin) => {
                    self.navigator.set_origin(origin);
                    let ran = self.plugin.run_command(&id, "");
                    self.navigator.set_origin(None);
                    match ran {
                        Ok(Ok(_)) => {}
                        Ok(Err(message)) => {
                            tracing::warn!(%message, "a plugin's command failed");
                            failed = Some((None, format!("Its command failed: {message}")));
                        }
                        Err(error) => {
                            tracing::warn!(%error, "a plugin failed to run a command");
                            failed = Some((None, failure_message(&error, Doing::Running)));
                        }
                    }
                }
                Job::Redraw => {
                    acted = false;
                    self.redraw_queued.store(false, Ordering::Relaxed);
                }
            }
            let views = draws.then(|| draw(&self.plugin, &registrations));
            let outcome = Outcome {
                id: self.id.clone(),
                generation: self.generation,
                registrations: registered,
                refused_types,
                views,
                panel,
                rows,
                acted,
                health: self.plugin.health(),
                failed,
            };
            if self.outcomes.send(outcome).is_err() {
                return;
            }
            if let Some(wake) = &self.wake {
                wake();
            }
        }
    }
}

/// Register the object types the plugin adds, when it holds
/// `object-types`, and say why Noura didn't add any it refused.
fn register_object_types(plugin: &RunningPlugin) -> Vec<String> {
    let registered = match plugin.register_object_types() {
        Ok(registered) => registered,
        Err(PluginError::CapabilityNotDeclared { .. } | PluginError::NotExported { .. }) => {
            return Vec::new();
        }
        Err(error) => {
            tracing::warn!(%error, "a plugin didn't list its object types");
            return vec!["It failed while listing its object types.".to_owned()];
        }
    };
    registered
        .into_iter()
        .filter_map(|(object_type, result)| {
            result.err().map(|error| {
                tracing::warn!(code = error.code(), "a plugin's type wasn't added");
                format!("Noura didn't add the type {object_type}: {error}")
            })
        })
        .collect()
}

/// The properties panel of the object `object`, as the plugin draws it.
fn draw_panel(plugin: &RunningPlugin, object: &str) -> PluginView {
    match plugin.render_panel(object) {
        Ok(nodes) => checked(nodes, "in a properties panel"),
        Err(error) => {
            tracing::warn!(%error, "a plugin failed to draw a properties panel");
            Err(failure_message(&error, Doing::Drawing))
        }
    }
}

/// The view `nodes` make, or why it can't be drawn. Only a page has the
/// `rows` that fetch a virtual list's rows, so a view `elsewhere` can't
/// hold one.
fn checked(nodes: Vec<plugin_host::Node>, elsewhere: &str) -> PluginView {
    let view = View::from_nodes(nodes).map_err(|error| error.to_string())?;
    match super::virtual_list_in(view.root()) {
        Some(list) => Err(format!("The virtual list {list} can't show {elsewhere}.")),
        None => Ok(view),
    }
}

/// What the plugin registers under each capability it holds. A list the
/// plugin can't give, because it lacks the capability or failed, is
/// empty.
fn register(plugin: &RunningPlugin) -> Registrations {
    let mut pages = listed(plugin.pages());
    pages.retain(valid_page);
    dedup_by(&mut pages, |page| page.id.clone());
    let mut commands = listed(plugin.commands());
    commands.retain(|command| command.palette && valid_command(command));
    dedup_by(&mut commands, |command| command.id.clone());
    let mut sections = listed(plugin.inbox_sections());
    sections.retain(|section| valid_id(&section.id) && valid_title(&section.title));
    dedup_by(&mut sections, |section| section.id.clone());
    let settings = match plugin.settings_page() {
        Ok(title) => title.filter(|title| valid_title(title)),
        Err(error) => {
            warn_unless_declined(&error);
            None
        }
    };
    let mut tools: Vec<ToolEntry> = listed(plugin.tools())
        .into_iter()
        .filter_map(|tool| {
            if !valid_tool_name(&tool.name) || tool.description.trim().is_empty() {
                tracing::warn!("a plugin tool without a usable name or description was left out");
                return None;
            }
            let schema: serde_json::Value = serde_json::from_str(&tool.input_schema).ok()?;
            match jsonschema::validator_for(&schema) {
                Ok(validator) => Some(ToolEntry {
                    name: tool.name,
                    description: tool.description,
                    input_schema: tool.input_schema,
                    validator: Arc::new(validator),
                }),
                Err(error) => {
                    tracing::warn!(error = %error.masked(), "a plugin tool's schema isn't valid");
                    None
                }
            }
        })
        .collect();
    dedup_by(&mut tools, |tool| tool.name.clone());
    Registrations {
        pages,
        commands,
        sections,
        settings,
        tools,
    }
}

fn listed<T>(result: Result<Vec<T>, PluginError>) -> Vec<T> {
    result.unwrap_or_else(|error| {
        warn_unless_declined(&error);
        Vec::new()
    })
}

/// Note a failed listing, unless the plugin just doesn't register
/// anything there.
fn warn_unless_declined(error: &PluginError) {
    if !matches!(
        error,
        PluginError::CapabilityNotDeclared { .. } | PluginError::NotExported { .. }
    ) {
        tracing::warn!(%error, "a plugin didn't list what it registers");
    }
}

/// Keep the first item with each key.
fn dedup_by<T>(items: &mut Vec<T>, key: impl Fn(&T) -> String) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(key(item)));
}

/// Draw every view the plugin registered. A page whose sidebar section
/// holds no nodes adds no section.
fn draw(plugin: &RunningPlugin, registrations: &Registrations) -> BTreeMap<Surface, PluginView> {
    registrations
        .surfaces()
        .into_iter()
        .filter_map(|surface| {
            let view = match plugin.render_surface(&surface) {
                Ok(nodes) => match &surface {
                    Surface::Page(_) => View::from_nodes(nodes).map_err(|error| error.to_string()),
                    Surface::Sidebar(_) if nodes.is_empty() => return None,
                    Surface::Sidebar(_) => checked(nodes, "in the sidebar"),
                    Surface::InboxSection(_) => checked(nodes, "in an Inbox section"),
                    Surface::Settings => checked(nodes, "on a page in Settings"),
                },
                Err(error) => {
                    tracing::warn!(%error, "a plugin failed to draw a view");
                    Err(failure_message(&error, Doing::Drawing))
                }
            };
            Some((surface, view))
        })
        .collect()
}

/// Why a plugin's call failed, in words for the place its view would be.
pub(super) fn failure_message(error: &PluginError, doing: Doing) -> String {
    match error {
        PluginError::Deadline { deadline } => format!(
            "The plugin ran longer than its time limit of {}, so Noura stopped it.",
            seconds(*deadline)
        ),
        PluginError::MemoryCap { cap_bytes } => format!(
            "The plugin needed more than its {} MB of memory, so Noura stopped it.",
            cap_bytes >> 20
        ),
        PluginError::Stopped => {
            "The plugin failed too often and stopped. Start it again in Settings, under Plugins."
                .to_owned()
        }
        PluginError::Busy { .. } => "The plugin was busy. Try again.".to_owned(),
        PluginError::CapabilityNotDeclared { capability } => {
            format!("The plugin isn't allowed the {capability} capability.")
        }
        PluginError::NotExported { .. } => "The plugin doesn't offer this.".to_owned(),
        PluginError::Instantiate(_) | PluginError::Call(_) => match doing {
            Doing::Drawing => "The plugin failed while drawing this view.",
            Doing::Handling => "The plugin failed while handling what you did.",
            Doing::Running => "The plugin failed while running it.",
            Doing::Listing => "The plugin failed while listing these rows.",
        }
        .to_owned(),
    }
}

/// A duration in words, such as "1 second".
fn seconds(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis.is_multiple_of(1000) {
        match millis / 1000 {
            1 => "1 second".to_owned(),
            seconds => format!("{seconds} seconds"),
        }
    } else {
        format!("{millis} ms")
    }
}

/// Whether a plugin's page has an ID and a title the rail can show.
fn valid_page(page: &Page) -> bool {
    let ok = valid_id(&page.id) && valid_title(&page.title);
    if !ok {
        tracing::warn!("a plugin page without a usable ID or title was left out");
    }
    ok
}

fn valid_command(command: &Command) -> bool {
    let ok = plugin_host::valid_command_id(&command.id) && valid_title(&command.title);
    if !ok {
        tracing::warn!("a plugin command without a usable ID or title was left out");
    }
    ok
}

/// An ID without spaces or control characters, up to 64 bytes.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// A title that isn't blank, up to 80 characters, without control
/// characters.
fn valid_title(title: &str) -> bool {
    let title = title.trim();
    !title.is_empty() && title.chars().count() <= 80 && !title.chars().any(char::is_control)
}

/// A tool name: a lowercase letter, then lowercase letters, digits,
/// hyphens or underscores, up to 64 characters.
fn valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}
