//! The Inbox page: today's greeting, Needs attention, the sections enabled
//! plugins add, and Recent. `app-core` builds what it shows; this draws it
//! and follows the links you choose.

use std::time::Duration;

use app_core::{App, FileLink, Inbox, InboxSection, OpenAs, ViewKind};
use egui::{
    Align, Frame, Id, Label, Layout, Margin, Rect, Response, RichText, Sense, Stroke, Ui,
    UiBuilder, accesskit::Role, pos2, vec2,
};

use crate::{
    icons, plugin_view,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, Weight, font, radius},
};

/// The size of the greeting at the top.
const GREETING_SIZE: f32 = 16.0;
/// The widest the Inbox's column grows, padding included.
const COLUMN_WIDTH: f32 = 672.0 - 48.0;
/// The space between the Inbox's parts.
const GAP: f32 = 20.0;

/// A link you chose, and how to open what it names.
enum Follow {
    Link(FileLink, OpenAs),
    Note(String, OpenAs),
}

/// The Inbox in the window whose ID salt is `window`.
pub(crate) fn show(ui: &mut Ui, app: &mut App, window: Id) {
    let inbox = app.inbox();
    // The greeting changes with the hour.
    ui.ctx().request_repaint_after(Duration::from_secs(60));
    let available = ui.available_rect_before_wrap();
    let width = available.width().min(COLUMN_WIDTH);
    let column = Rect::from_min_size(
        pos2(available.center().x - width / 2.0, available.top()),
        vec2(width, available.height()),
    );
    let builder = UiBuilder::new()
        .max_rect(column)
        .layout(Layout::top_down(Align::Min));
    let follow = ui
        .scope_builder(builder, |ui| page(ui, app, window, &inbox))
        .inner;
    match follow {
        Some(Follow::Link(link, how)) => app.follow_link(&link, how),
        Some(Follow::Note(path, how)) => app.open_file(&path, how),
        None => {}
    }
}

fn page(ui: &mut Ui, app: &mut App, window: Id, inbox: &Inbox) -> Option<Follow> {
    let mut follow = None;
    heading(ui, inbox.greeting, 0, GREETING_SIZE);
    ui.add_space(4.0);
    if !inbox.needs_attention.is_empty() {
        ui.add_space(GAP);
        follow = follow.or(needs_attention(ui, inbox));
    }
    for section in &inbox.sections {
        ui.add_space(GAP);
        plugin_section(ui, app, window, section);
    }
    ui.add_space(GAP);
    follow.or(recent(ui, inbox))
}

/// Draw `text` as a heading at `level`, counted from 0 for the page's own.
fn heading(ui: &mut Ui, text: &str, level: usize, size: f32) {
    let weight = if level == 0 {
        Weight::Semibold
    } else {
        Weight::Medium
    };
    let response = ui.label(RichText::new(text).font(font(ui.ctx(), size, weight)));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Heading);
        node.set_level(level);
        node.clear_value();
        node.set_label(text);
    });
}

/// A bordered box that holds a part's rows.
fn rows<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(radius(1.0))
        .inner_margin(Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// How a click on a link opens what it names, as a click in the file tree
/// does: a preview, or a kept tab with Cmd-click (Ctrl-click on Windows
/// and Linux) or a middle-click. A double-click can't keep the tab here as
/// it does in the tree: the first click's preview takes the Inbox's place,
/// so the second lands on the note.
fn opened_as(ui: &Ui, response: &Response) -> Option<OpenAs> {
    if response.middle_clicked() {
        Some(OpenAs::Kept)
    } else if response.clicked() {
        let command = ui.input(|input| input.modifiers.command);
        Some(if command {
            OpenAs::Kept
        } else {
            OpenAs::Preview
        })
    } else {
        None
    }
}

fn needs_attention(ui: &mut Ui, inbox: &Inbox) -> Option<Follow> {
    let palette = Palette::of(ui.ctx());
    let mut follow = None;
    heading(ui, "Needs attention", 1, BODY_SIZE);
    ui.add_space(8.0);
    let list = UiBuilder::new()
        .accessibility_role(Role::List)
        .accessibility_label("Needs attention");
    ui.scope_builder(list, |ui| {
        rows(ui, |ui| {
            for (index, item) in inbox.needs_attention.iter().enumerate() {
                if index > 0 {
                    ui.separator();
                }
                let row = UiBuilder::new()
                    .accessibility_role(Role::ListItem)
                    .accessibility_label(item.message.clone());
                ui.scope_builder(row, |ui| {
                    ui.add(Label::new(RichText::new(&item.message).size(BODY_SIZE)).wrap());
                    ui.horizontal_wrapped(|ui| {
                        for link in &item.links {
                            let text = RichText::new(&link.label)
                                .size(SUPPORTING_SIZE)
                                .color(palette.muted_foreground);
                            let response = ui.link(text);
                            if let Some(how) = opened_as(ui, &response) {
                                follow = Some(Follow::Link(link.clone(), how));
                            }
                        }
                    });
                });
            }
        });
    });
    follow
}

/// A plugin's section: the view it drew, or why it couldn't load or draw,
/// in its own place. What you do there goes back to the plugin.
fn plugin_section(ui: &mut Ui, app: &mut App, window: Id, section: &InboxSection) {
    // A section that draws its own heading, with its icon and buttons,
    // gets no other.
    let own_heading = section.content.as_ref().is_ok_and(|view| {
        view.root()
            .children
            .iter()
            .any(|child| matches!(child.kind, ViewKind::Heading { .. }))
    });
    if !own_heading {
        heading(ui, &section.title, 1, BODY_SIZE);
        ui.add_space(8.0);
    }
    let empty = section
        .content
        .as_ref()
        .is_ok_and(|view| view.root().children.is_empty());
    if empty {
        return;
    }
    let place = (section.plugin_id.as_str(), section.section_id.as_str());
    plugin_view::show_section(ui, app, window, place, &section.content);
}

fn recent(ui: &mut Ui, inbox: &Inbox) -> Option<Follow> {
    ui.horizontal(|ui| {
        let palette = Palette::of(ui.ctx());
        let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(
            ui.painter(),
            icons::NEW_NOTE,
            rect,
            palette.muted_foreground,
            false,
        );
        heading(ui, "Recent", 1, BODY_SIZE);
    });
    ui.add_space(8.0);
    if inbox.recent.is_empty() {
        rows(ui, |ui| {
            let palette = Palette::of(ui.ctx());
            ui.label(
                RichText::new("No notes yet.")
                    .size(BODY_SIZE)
                    .color(palette.muted_foreground),
            );
        });
        return None;
    }
    let palette = Palette::of(ui.ctx());
    let mut follow = None;
    // The legacy list: a bordered card of 40 px rows with lines between,
    // each filling on hover.
    Frame::new()
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(radius(1.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let count = inbox.recent.len();
            for (index, note) in inbox.recent.iter().enumerate() {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                // Notes that share a title show their paths to tell them
                // apart.
                let label = if note.shares_title {
                    format!("{}, {}", note.title, note.relative_path)
                } else {
                    note.title.clone()
                };
                response.widget_info(|| egui::WidgetInfo::labeled(Role::Link, true, &label));
                let path = note.relative_path.clone();
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_description(path);
                });
                if ui.is_rect_visible(rect) {
                    let painter = ui.painter();
                    if response.hovered() {
                        let corner = egui::CornerRadius {
                            nw: if index == 0 { 10 } else { 0 },
                            ne: if index == 0 { 10 } else { 0 },
                            sw: if index + 1 == count { 10 } else { 0 },
                            se: if index + 1 == count { 10 } else { 0 },
                        };
                        painter.rect_filled(rect.shrink(1.0), corner, palette.accent);
                    }
                    if index > 0 {
                        painter.hline(
                            rect.x_range(),
                            rect.top(),
                            Stroke::new(1.0, palette.border.gamma_multiply(0.6)),
                        );
                    }
                    let title = painter.layout_no_wrap(
                        note.title.clone(),
                        egui::FontId::proportional(BODY_SIZE),
                        palette.foreground,
                    );
                    let at = pos2(rect.left() + 16.0, rect.center().y - title.size().y / 2.0);
                    let end = at.x + title.size().x;
                    painter.galley(at, title, palette.foreground);
                    if note.shares_title {
                        let path = painter.layout_no_wrap(
                            note.relative_path.clone(),
                            egui::FontId::proportional(SUPPORTING_SIZE),
                            palette.muted_foreground,
                        );
                        let at = pos2(end + 8.0, rect.center().y - path.size().y / 2.0);
                        let shown = Rect::from_min_size(at, path.size());
                        painter.galley(at, path, palette.muted_foreground);
                        // The path reads on its own too, as the legacy
                        // label beside the title did.
                        let label = ui.interact(shown, response.id.with("path"), Sense::hover());
                        let text = note.relative_path.clone();
                        label.widget_info(|| egui::WidgetInfo::labeled(Role::Label, true, &text));
                    }
                    if response.has_focus() {
                        crate::common::focus_ring(painter, rect.shrink(2.0), radius(0.8), &palette);
                    }
                }
                let response = response.on_hover_text(&note.relative_path);
                if let Some(how) = opened_as(ui, &response) {
                    follow = Some(Follow::Note(note.relative_path.clone(), how));
                }
            }
        });
    follow
}
