//! Plugin views: a plugin page, an Inbox section, a plugin's page in
//! Settings or a properties panel, drawn from the nodes the plugin
//! returned with the kit's widgets and the theme's tokens, or the reason
//! it can't be drawn in its place. Presses, selections, header presses,
//! menu choices, input changes, submits and drops go back to the plugin by
//! node ID.
//!
//! What the widgets remember between frames, such as a list's selection,
//! an open menu or the text you typed, lives in egui's memory under IDs
//! made from the window, the place the view shows, and the node ID. A redraw with
//! the same node IDs keeps it, and each window that shows the view keeps
//! its own: every window draws with one egui context and shares its
//! memory.

mod board;
mod calendar;
mod conversation;
mod drag;
mod form;
mod heading;
mod menu;
mod rows;

use std::ops::Range;

use app_core::{App, PluginEvent, PluginPlace, PluginView, TabId, ViewKind, ViewNode, VirtualRow};
use egui::{Frame, Id, Margin, RichText, Stroke, Ui, UiBuilder, accesskit::Role};

use crate::{
    common::muted,
    markdown,
    theme::{Palette, radius},
};

/// Two panes side by side, as the legacy Tasks page's list and detail:
/// they fill the rest of the page from edge to edge, the first
/// `first_width` points wide with a line on its right, and each scrolls on
/// its own. A pane's panel draws no box.
fn split(ui: &mut Ui, node: &ViewNode, first_width: u32, kit: &mut Kit) {
    let palette = Palette::of(ui.ctx());
    // The page keeps 24 points around its contents; the panes run past
    // them to the page's edges, as the legacy panes did.
    // The gap above it is empty, so the panes start where the content
    // above them ends.
    let mut available = ui.available_rect_before_wrap();
    available.min.y -= ui.spacing().item_spacing.y;
    let height = ui
        .clip_rect()
        .bottom()
        .min(available.bottom() + 24.0)
        .max(available.top() + 240.0)
        - available.top();
    let rect = egui::Rect::from_min_size(
        available.min - egui::vec2(24.0, 0.0),
        egui::vec2(available.width() + 48.0, height),
    );
    // Only the page's own width counts toward its size, so the panes
    // running into its margins never widen it.
    ui.allocate_rect(
        egui::Rect::from_min_size(available.min, egui::vec2(available.width(), height)),
        egui::Sense::hover(),
    );
    let first = (first_width as f32).min(rect.width() / 2.0);
    let line = rect.left() + first;
    ui.painter()
        .vline(line, rect.y_range(), Stroke::new(1.0, palette.border));
    let panes = [
        egui::Rect::from_min_max(rect.min, egui::pos2(line, rect.bottom())),
        egui::Rect::from_min_max(egui::pos2(line + 1.0, rect.top()), rect.max),
    ];
    for (index, (pane, child)) in panes.into_iter().zip(&node.children).enumerate() {
        let mut pane_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(pane)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        egui::ScrollArea::vertical()
            .id_salt((&node.id, index))
            .auto_shrink(false)
            .show(&mut pane_ui, |ui| {
                Frame::new().inner_margin(Margin::same(8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let builder = UiBuilder::new().accessibility_role(Role::Group);
                    let builder = match &child.kind {
                        ViewKind::Panel { label: Some(label) } => {
                            builder.accessibility_label(label)
                        }
                        _ => builder,
                    };
                    // A pane is narrow, so its nodes stack, one per row.
                    ui.push_id(&child.id, |ui| {
                        ui.scope_builder(builder, |ui| {
                            ui.spacing_mut().item_spacing.y = 12.0;
                            for node in &child.children {
                                self::node(ui, node, kit);
                            }
                        });
                    });
                });
            });
    }
}

/// A title over a group in the sidebar, as the legacy sidebar's section
/// labels: 12 px medium in the faded foreground, after a caret.
fn sidebar_label(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        ui.set_height(32.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add_space(8.0);
        let color = palette.foreground.gamma_multiply(0.7);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        crate::icons::paint(ui.painter(), crate::icons::CARET_DOWN, rect, color, false);
        ui.label(
            RichText::new(text)
                .font(crate::theme::font(
                    ui.ctx(),
                    crate::theme::SUPPORTING_SIZE,
                    crate::theme::Weight::Medium,
                ))
                .color(color),
        );
    });
}

/// Where a plugin's view shows.
#[derive(Debug, Clone, Copy, Hash)]
enum Place<'a> {
    /// The rail page with this ID.
    Page(&'a str),
    /// The sidebar section of the rail page with this ID.
    Sidebar(&'a str),
    /// The properties panel of the object open in this tab.
    Panel(TabId),
    /// The Inbox section of a plugin: (plugin ID, section ID).
    Section(&'a str, &'a str),
    /// A plugin's page in Settings, by its section ID.
    Settings(&'a str),
}

/// The view being drawn and what its widgets ask for.
struct Kit<'a> {
    /// The window's salt for IDs.
    window: Id,
    place: Place<'a>,
    app: &'a mut App,
    /// What happened in the view this frame, oldest first.
    events: Vec<PluginEvent>,
    /// The link clicked in a Markdown node.
    link: Option<String>,
    /// Enter left a single-line text input this frame, which submits the
    /// form around it.
    entered: bool,
    /// How many panels the node being drawn sits in.
    depth: usize,
    /// The input being drawn is a one-line form's field, whose label is
    /// its placeholder.
    inline: bool,
    /// The page's heading has a detail line, such as the month a
    /// calendar shows, so a calendar grid adds no title of its own.
    page_detail: bool,
    /// The node being drawn sits in a conversation, where text is a quiet
    /// line and buttons are small.
    quiet: bool,
}

impl Kit<'_> {
    /// The ID of the widget that draws the node `node` of this view in
    /// this window.
    fn id(&self, node: &str) -> Id {
        Id::unique(("plugin-node", self.window, self.place, node))
    }

    /// The rows in `range` of the virtual list `list`, each `None` until
    /// the plugin returns it.
    fn rows(&mut self, list: &str, range: Range<u32>) -> Result<Vec<Option<VirtualRow>>, String> {
        let elsewhere = match self.place {
            Place::Page(page) => return self.app.plugin_rows(page, list, range),
            // App-core refuses these views when they hold a virtual list.
            Place::Sidebar(_) => "in the sidebar",
            Place::Panel(_) => "in a properties panel",
            Place::Section(..) => "in an Inbox section",
            Place::Settings(_) => "on a page in Settings",
        };
        Err(format!("The virtual list {list} can't show {elsewhere}."))
    }
}

impl Place<'_> {
    /// Where `app-core` knows this view to be.
    fn plugin_place(self) -> PluginPlace {
        match self {
            Self::Page(page) => PluginPlace::Page(page.to_owned()),
            Self::Sidebar(page) => PluginPlace::Sidebar(page.to_owned()),
            Self::Panel(tab) => PluginPlace::Panel(tab),
            Self::Section(plugin, section) => PluginPlace::Inbox {
                plugin: plugin.to_owned(),
                section: section.to_owned(),
            },
            Self::Settings(section) => PluginPlace::Settings(section.to_owned()),
        }
    }
}

/// The section the plugin page `page_id` adds to the sidebar, when it
/// adds one, in the window whose ID salt is `window`. Returns whether it
/// drew one.
pub(crate) fn show_sidebar(ui: &mut Ui, app: &mut App, window: Id, page_id: &str) -> bool {
    let Some(view) = app.plugin_sidebar(page_id) else {
        return false;
    };
    let (events, link) = draw(ui, app, window, Place::Sidebar(page_id), &view);
    for event in &events {
        app.send_plugin_sidebar_event(page_id, event);
    }
    if let Some(url) = link {
        app.open_link(&url);
    }
    true
}

/// Whether the plugin page `page_id` starts with a page heading of its
/// own.
pub(crate) fn has_page_heading(app: &App, page_id: &str) -> bool {
    app.plugin_view(page_id)
        .and_then(|view| view.ok())
        .is_some_and(|view| {
            view.root().children.first().is_some_and(|child| {
                matches!(
                    child.kind,
                    ViewKind::Heading {
                        level: app_core::HeadingLevel::Page,
                        ..
                    }
                )
            })
        })
}

/// The plugin page `page_id` in the window whose ID salt is `window`.
pub(crate) fn show(ui: &mut Ui, app: &mut App, window: Id, page_id: &str) {
    let Some(view) = app.plugin_view(page_id) else {
        muted(ui, "Its plugin isn't running.");
        return;
    };
    let (events, link) = draw(ui, app, window, Place::Page(page_id), &view);
    for event in &events {
        app.send_plugin_event(page_id, event);
    }
    if let Some(url) = link {
        app.open_link(&url);
    }
}

/// The properties panel a plugin draws beside the body of an object of its
/// own type, in the note screen's panel slot of the window whose ID salt
/// is `window`. Events go back to the plugin by node ID.
pub(crate) fn show_panel(ui: &mut Ui, app: &mut App, window: Id, tab: TabId, view: &PluginView) {
    let (events, link) = draw(ui, app, window, Place::Panel(tab), view);
    for event in &events {
        app.send_panel_event(tab, event);
    }
    if let Some(url) = link {
        app.open_link(&url);
    }
}

/// A plugin's section in the Inbox, in the window whose ID salt is
/// `window`. Events go back to the plugin by node ID.
pub(crate) fn show_section(
    ui: &mut Ui,
    app: &mut App,
    window: Id,
    (plugin, section): (&str, &str),
    view: &PluginView,
) {
    let (events, link) = draw(ui, app, window, Place::Section(plugin, section), view);
    for event in &events {
        app.send_inbox_event(plugin, section, event);
    }
    if let Some(url) = link {
        app.open_link(&url);
    }
}

/// A plugin's page in Settings, by its section ID, in the window whose ID
/// salt is `window`. Events go back to the plugin by node ID.
pub(crate) fn show_settings(ui: &mut Ui, app: &mut App, window: Id, section_id: &str) {
    let Some(view) = app.plugin_settings_view(section_id) else {
        muted(ui, "Its plugin isn't running.");
        return;
    };
    let (events, link) = draw(ui, app, window, Place::Settings(section_id), &view);
    for event in &events {
        app.send_plugin_settings_event(section_id, event);
    }
    if let Some(url) = link {
        app.open_link(&url);
    }
}

/// Draw `view` at `place`, or the reason it can't be drawn, and return
/// what happened in it this frame, oldest first, and the link clicked.
fn draw(
    ui: &mut Ui,
    app: &mut App,
    window: Id,
    place: Place,
    view: &PluginView,
) -> (Vec<PluginEvent>, Option<String>) {
    let view = match view {
        Err(reason) => {
            problem(ui, reason);
            return (Vec::new(), None);
        }
        Ok(view) => view,
    };
    let mut kit = Kit {
        window,
        place,
        app,
        events: Vec::new(),
        link: None,
        entered: false,
        depth: 0,
        inline: false,
        page_detail: false,
        quiet: false,
    };
    ui.push_id(("plugin-view", window, place), |ui| {
        node(ui, view.root(), &mut kit);
    });
    (kit.events, kit.link)
}

/// The reason a view can't be drawn, where the view would be.
fn problem(ui: &mut Ui, reason: &str) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Alert)
        .accessibility_label(reason);
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Noura can't show this")
                        .strong()
                        .color(palette.destructive),
                );
                ui.label(reason);
            });
    });
}

/// A titled box, as panels and forms draw.
fn boxed(ui: &mut Ui, role: Role, label: Option<&str>, add: impl FnOnce(&mut Ui)) {
    let mut builder = UiBuilder::new().accessibility_role(role);
    if let Some(label) = label {
        builder = builder.accessibility_label(label);
    }
    let palette = Palette::of(ui.ctx());
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical(add);
            });
    });
}

/// Whether `node` lines up beside its neighbours, as a toolbar's buttons
/// and a segmented choice do.
fn inline(node: &ViewNode) -> bool {
    match &node.kind {
        ViewKind::Button { .. } => true,
        ViewKind::Input { value, choices, .. } => form::segmented(value, choices),
        _ => false,
    }
}

/// Draw a panel's children from top to bottom, with each run of inline
/// nodes on one row.
fn children(ui: &mut Ui, nodes: &[ViewNode], kit: &mut Kit) {
    let mut index = 0;
    while index < nodes.len() {
        let run = nodes[index..]
            .iter()
            .take_while(|node| inline(node))
            .count();
        if run > 1 {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for child in &nodes[index..index + run] {
                    self::node(ui, child, kit);
                }
            });
            index += run;
        } else {
            let sidebar = matches!(kit.place, Place::Sidebar(_));
            // In the sidebar, text over a list titles it, and a list that
            // no text titles shows its own label as its title, as the
            // legacy sidebar's Views and Folders did.
            let list_next = matches!(
                nodes.get(index + 1).map(|node| &node.kind),
                Some(ViewKind::List { .. })
            );
            match &nodes[index].kind {
                ViewKind::Text(text) if sidebar && list_next => {
                    sidebar_label(ui, text);
                }
                ViewKind::List { label, .. } => {
                    let titled = index > 0 && matches!(nodes[index - 1].kind, ViewKind::Text(_));
                    if sidebar && !titled {
                        sidebar_label(ui, label);
                    }
                    self::node(ui, &nodes[index], kit);
                }
                _ => self::node(ui, &nodes[index], kit),
            }
            index += 1;
        }
    }
}

fn node(ui: &mut Ui, node: &ViewNode, kit: &mut Kit) {
    ui.push_id(&node.id, |ui| match &node.kind {
        ViewKind::Panel { label } => {
            kit.depth += 1;
            if kit.depth == 1 {
                // The view's own panel is the page, so it draws no box.
                let mut builder = UiBuilder::new().accessibility_role(Role::Group);
                if let Some(label) = label.as_deref() {
                    builder = builder.accessibility_label(label);
                }
                ui.scope_builder(builder, |ui| {
                    ui.set_width(ui.available_width());
                    ui.vertical(|ui| children(ui, &node.children, kit));
                });
            } else {
                boxed(ui, Role::Group, label.as_deref(), |ui| {
                    children(ui, &node.children, kit);
                });
            }
            kit.depth -= 1;
        }
        ViewKind::Text(text) if matches!(kit.place, Place::Sidebar(_)) => {
            // Other text in the sidebar is a muted line, as the legacy
            // sidebar's "No chats yet."
            let palette = Palette::of(ui.ctx());
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.label(
                    RichText::new(text)
                        .size(crate::theme::BODY_SIZE)
                        .color(palette.muted_foreground),
                );
            });
        }
        ViewKind::Text(text) if kit.quiet => conversation::quiet_line(ui, text),
        ViewKind::Text(text) => {
            ui.label(text);
        }
        ViewKind::Markdown(text) => {
            if let Some(url) = markdown::show(ui, text) {
                kit.link = Some(url);
            }
        }
        ViewKind::Button { label } => menu::button(ui, node, label, kit),
        ViewKind::Heading { .. } => heading::heading(ui, node, kit),
        ViewKind::Empty { .. } => heading::empty(ui, node, kit),
        ViewKind::List {
            label,
            reorder,
            selected,
            opens_on_click,
        } => {
            let options = rows::ListOptions {
                label,
                reorder: *reorder,
                selected: selected.as_deref(),
                opens_on_click: *opens_on_click,
            };
            rows::list(ui, node, &options, kit);
        }
        ViewKind::Board { label } => board::board(ui, node, label, kit),
        ViewKind::Split { first_width } => split(ui, node, *first_width, kit),
        ViewKind::Conversation { label } => conversation::conversation(ui, node, label, kit),
        ViewKind::Message { .. } => conversation::message(ui, node, kit),
        ViewKind::Activity { .. } => conversation::activity(ui, node, kit),
        ViewKind::Composer {
            label,
            value,
            send,
            stop,
        } => {
            let parts = conversation::ComposerParts {
                label,
                value,
                send,
                stop: stop.as_deref(),
            };
            conversation::composer(ui, node, &parts, kit);
        }
        ViewKind::Calendar { label, mode, date } => {
            calendar::calendar(ui, node, label, *mode, date, kit);
        }
        ViewKind::VirtualList { label, rows } => {
            rows::virtual_list(ui, node, label, *rows, kit);
        }
        ViewKind::Table { label, columns } => rows::table(ui, node, label, columns, kit),
        ViewKind::Form {
            label,
            submit,
            required,
        } => form::form(ui, node, (label, submit, required), kit),
        ViewKind::Input { .. } => form::input(ui, node, kit),
        ViewKind::NameField { label, value } => form::name_field(ui, node, label, value, kit),
        // Their list, table, board or menu draws them.
        ViewKind::Item { .. }
        | ViewKind::Row { .. }
        | ViewKind::BoardColumn { .. }
        | ViewKind::Card { .. }
        | ViewKind::Entry { .. }
        | ViewKind::Menu { .. }
        | ViewKind::MenuItem { .. }
        | ViewKind::Separator => {}
    });
}
