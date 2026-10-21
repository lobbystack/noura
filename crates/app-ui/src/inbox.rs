//! The Inbox page: today's greeting, Needs attention, the sections enabled
//! plugins add, and Recent. `app-core` builds what it shows; this draws it
//! and follows the links you choose.

use std::time::Duration;

use app_core::{App, FileLink, Inbox, InboxSection, OpenAs};
use egui::{
    Align, Frame, Label, Layout, Margin, Rect, Response, RichText, Stroke, Ui, UiBuilder,
    accesskit::Role, pos2, vec2,
};

use crate::{
    common::muted,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, TITLE_SIZE, radius},
};

/// The widest the Inbox's column grows.
const COLUMN_WIDTH: f32 = 640.0;
/// The space between the Inbox's parts.
const GAP: f32 = 20.0;

/// A link you chose, and how to open what it names.
enum Follow {
    Link(FileLink, OpenAs),
    Note(String, OpenAs),
}

pub(crate) fn show(ui: &mut Ui, app: &mut App) {
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
    let follow = ui.scope_builder(builder, |ui| page(ui, &inbox)).inner;
    match follow {
        Some(Follow::Link(link, how)) => app.follow_link(&link, how),
        Some(Follow::Note(path, how)) => app.open_file(&path, how),
        None => {}
    }
}

fn page(ui: &mut Ui, inbox: &Inbox) -> Option<Follow> {
    let mut follow = None;
    heading(ui, inbox.greeting, 0, TITLE_SIZE);
    if !inbox.needs_attention.is_empty() {
        ui.add_space(GAP);
        follow = follow.or(needs_attention(ui, inbox));
    }
    for section in &inbox.sections {
        ui.add_space(GAP);
        plugin_section(ui, section);
    }
    ui.add_space(GAP);
    follow.or(recent(ui, inbox))
}

/// Draw `text` as a heading at `level`, counted from 0 for the page's own.
fn heading(ui: &mut Ui, text: &str, level: usize, size: f32) {
    let response = ui.label(RichText::new(text).size(size).strong());
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
        .inner_margin(Margin::symmetric(16, 10))
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

/// A plugin's section: its lines, or why they couldn't load.
fn plugin_section(ui: &mut Ui, section: &InboxSection) {
    heading(ui, &section.title, 1, BODY_SIZE);
    ui.add_space(8.0);
    match &section.content {
        Ok(lines) if lines.is_empty() => {}
        Ok(lines) => rows(ui, |ui| {
            for line in lines {
                ui.add(Label::new(RichText::new(line).size(BODY_SIZE)).wrap());
            }
        }),
        Err(message) => {
            let palette = Palette::of(ui.ctx());
            let alert = UiBuilder::new()
                .accessibility_role(Role::Alert)
                .accessibility_label(message.clone());
            ui.scope_builder(alert, |ui| {
                rows(ui, |ui| {
                    ui.add(
                        Label::new(
                            RichText::new(message)
                                .size(BODY_SIZE)
                                .color(palette.destructive),
                        )
                        .wrap(),
                    );
                });
            });
        }
    }
}

fn recent(ui: &mut Ui, inbox: &Inbox) -> Option<Follow> {
    heading(ui, "Recent", 1, BODY_SIZE);
    ui.add_space(8.0);
    if inbox.recent.is_empty() {
        muted(ui, "No notes yet.");
        return None;
    }
    let palette = Palette::of(ui.ctx());
    let mut follow = None;
    rows(ui, |ui| {
        for note in &inbox.recent {
            // Notes that share a title show their paths to tell them apart.
            let response = ui
                .horizontal_wrapped(|ui| {
                    let response = ui.link(RichText::new(&note.title).size(BODY_SIZE));
                    if note.shares_title {
                        ui.label(
                            RichText::new(&note.relative_path)
                                .size(SUPPORTING_SIZE)
                                .color(palette.muted_foreground),
                        );
                    }
                    response
                })
                .inner;
            let label = if note.shares_title {
                format!("{}, {}", note.title, note.relative_path)
            } else {
                note.title.clone()
            };
            let path = note.relative_path.clone();
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_label(label);
                node.set_description(path);
            });
            let response = response.on_hover_text(&note.relative_path);
            if let Some(how) = opened_as(ui, &response) {
                follow = Some(Follow::Note(note.relative_path.clone(), how));
            }
        }
    });
    follow
}
