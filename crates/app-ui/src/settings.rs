//! Settings: a dialog that lists `app-core`'s sections under their
//! groups on the left and shows the chosen section on the right. Escape
//! or Close closes it.

use app_core::{
    ABOUT_LINKS, ACTUAL_SIZE, App, Appearance, LICENSE, NoticeKind, Settings, TextSize, VERSION,
    ZOOM_LEVELS, settings_section,
};
use egui::{
    Button, Frame, Id, Key, Margin, Modal, RichText, ScrollArea, Stroke, TextEdit, Ui, UiBuilder,
    accesskit::Role, vec2,
};

use crate::{
    common::muted,
    theme::{Palette, TITLE_SIZE, radius},
};

const WIDTH: f32 = 760.0;
const HEIGHT: f32 = 520.0;
const NAV_WIDTH: f32 = 180.0;

/// What Settings remembers between frames.
#[derive(Default)]
pub(crate) struct SettingsView {
    /// Settings was open last frame.
    open: bool,
    /// The widget that had the keyboard before Settings opened.
    return_focus: Option<Id>,
    /// The name being typed in Name and location, once you start typing.
    name: Option<String>,
}

/// Draw Settings while it is open. Once it closes, the keyboard goes back
/// where it was, or to the rail's Settings button when nothing had it.
pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut SettingsView) {
    let Some(settings) = app.settings().cloned() else {
        if view.open {
            let focus = view
                .return_focus
                .take()
                .unwrap_or_else(crate::rail::settings_button_id);
            ui.ctx().memory_mut(|memory| memory.request_focus(focus));
        }
        *view = SettingsView::default();
        return;
    };
    let opened = !view.open;
    if opened {
        view.open = true;
        view.return_focus = ui.ctx().memory(|memory| memory.focused());
    }
    let mut close = false;
    let screen = ui.ctx().content_rect().size();
    let modal = Modal::new(Id::unique("settings")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::Dialog)
            .accessibility_label("Settings");
        ui.scope_builder(builder, |ui| {
            ui.set_width(WIDTH.min(screen.x - 64.0).max(320.0));
            ui.set_height(HEIGHT.min(screen.y - 96.0).max(240.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new("Settings").size(TITLE_SIZE).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.button("Close").clicked();
                });
            });
            ui.separator();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| nav(ui, app, &settings, opened));
                ui.separator();
                ScrollArea::vertical()
                    .id_salt(("settings-content", &settings.section))
                    .auto_shrink(false)
                    .show(ui, |ui| ui.vertical(|ui| content(ui, app, &settings, view)));
            });
        });
    });
    if close || modal.should_close() {
        app.close_settings();
    }
}

/// The section list: each group's title, then its sections.
fn nav(ui: &mut Ui, app: &mut App, settings: &Settings, opened: bool) {
    let mut chosen = None;
    let builder = UiBuilder::new()
        .accessibility_role(Role::Navigation)
        .accessibility_label("Settings sections");
    ui.scope_builder(builder, |ui| {
        ui.set_width(NAV_WIDTH);
        for (group, sections) in app.settings_sections() {
            let builder = UiBuilder::new()
                .accessibility_role(Role::Group)
                .accessibility_label(group.title());
            ui.scope_builder(builder, |ui| {
                muted(ui, group.title());
                for section in sections {
                    let selected = section.id == settings.section;
                    let button =
                        Button::selectable(selected, &section.title).min_size(vec2(NAV_WIDTH, 0.0));
                    let response = ui.add(button);
                    if selected && opened {
                        response.request_focus();
                    }
                    if response.clicked() {
                        chosen = Some(section.id.clone());
                    }
                }
            });
            ui.add_space(8.0);
        }
    });
    if let Some(id) = chosen {
        app.select_settings_section(&id);
    }
}

/// The chosen section's title, its notice and its controls.
fn content(ui: &mut Ui, app: &mut App, settings: &Settings, view: &mut SettingsView) {
    let title = app
        .settings_sections()
        .into_iter()
        .flat_map(|(_, sections)| sections)
        .find(|section| section.id == settings.section)
        .map(|section| section.title.clone())
        .unwrap_or_default();
    ui.label(RichText::new(title).size(TITLE_SIZE).strong());
    ui.add_space(4.0);
    notice(ui, settings);
    match settings.section.as_str() {
        settings_section::GENERAL => general(ui, app, settings),
        settings_section::EDITOR => editor(ui, app),
        settings_section::SHORTCUTS => crate::shortcuts::page(ui, app),
        settings_section::WORKSPACE => workspace(ui, app, view),
        settings_section::ABOUT => about(ui, app),
        settings_section::UPDATES => crate::updates::update_settings(ui, app),
        // Sections that later areas register draw themselves later.
        _ => {}
    }
}

/// What the last action in Settings did.
fn notice(ui: &mut Ui, settings: &Settings) {
    let Some(notice) = &settings.notice else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let (role, color) = match notice.kind {
        NoticeKind::Problem => (Role::Alert, palette.destructive),
        NoticeKind::Done => (Role::Status, palette.foreground),
    };
    let builder = UiBuilder::new().accessibility_role(role);
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&notice.title).strong().color(color));
                if !notice.message.is_empty() {
                    ui.label(&notice.message);
                }
            });
    });
    ui.add_space(8.0);
}

/// A titled group of controls, such as Appearance's three choices.
fn field(ui: &mut Ui, title: &str, role: Role, add: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    let builder = UiBuilder::new()
        .accessibility_role(role)
        .accessibility_label(title);
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(title).strong());
        add(ui);
    });
}

fn general(ui: &mut Ui, app: &mut App, settings: &Settings) {
    let preferences = *app.preferences();
    let mut appearance = preferences.appearance;
    field(ui, "Appearance", Role::RadioGroup, |ui| {
        ui.horizontal(|ui| {
            for choice in Appearance::ALL {
                ui.radio_value(&mut appearance, choice, choice.title());
            }
        });
    });
    if appearance != preferences.appearance {
        app.set_appearance(appearance);
    }

    let mut zoom = None;
    field(ui, "Zoom", Role::Group, |ui| {
        ui.horizontal(|ui| {
            let smallest = ZOOM_LEVELS[0];
            let largest = ZOOM_LEVELS[ZOOM_LEVELS.len() - 1];
            if ui
                .add_enabled(preferences.zoom > smallest, Button::new("Zoom out"))
                .clicked()
            {
                zoom = Some(Zoom::Out);
            }
            ui.label(format!("{}%", preferences.zoom));
            if ui
                .add_enabled(preferences.zoom < largest, Button::new("Zoom in"))
                .clicked()
            {
                zoom = Some(Zoom::In);
            }
            if ui
                .add_enabled(preferences.zoom != ACTUAL_SIZE, Button::new("Actual size"))
                .clicked()
            {
                zoom = Some(Zoom::Reset);
            }
        });
    });
    match zoom {
        Some(Zoom::In) => app.zoom_in(),
        Some(Zoom::Out) => app.zoom_out(),
        Some(Zoom::Reset) => app.actual_size(),
        None => {}
    }

    let login = &settings.open_at_login;
    let mut enabled = login.enabled.unwrap_or(false);
    field(ui, "Open at login", Role::Group, |ui| {
        let changed = ui
            .add_enabled(
                login.enabled.is_some(),
                egui::Checkbox::new(&mut enabled, "Open Noura when you sign in to this computer"),
            )
            .changed();
        if let Some(problem) = &login.problem {
            let palette = Palette::of(ui.ctx());
            let builder = UiBuilder::new().accessibility_role(Role::Alert);
            ui.scope_builder(builder, |ui| {
                ui.label(RichText::new(problem).color(palette.destructive));
            });
        }
        if changed {
            app.set_open_at_login(enabled);
        }
    });
}

enum Zoom {
    In,
    Out,
    Reset,
}

fn editor(ui: &mut Ui, app: &mut App) {
    let current = app.preferences().text_size;
    let mut text_size = current;
    field(ui, "Text size", Role::RadioGroup, |ui| {
        muted(ui, "Changes the text in notes on this computer.");
        ui.horizontal(|ui| {
            for choice in TextSize::ALL {
                ui.radio_value(&mut text_size, choice, choice.title());
            }
        });
    });
    if text_size != current {
        app.set_text_size(text_size);
    }
}

fn workspace(ui: &mut Ui, app: &mut App, view: &mut SettingsView) {
    let Some(workspace) = app.workspace() else {
        muted(ui, "Open a workspace to change its name.");
        return;
    };
    let current = workspace.name();
    let root = workspace.root().to_owned();
    let mut rename = false;
    let mut reveal = false;
    field(ui, "Name", Role::Group, |ui| {
        let name = view.name.get_or_insert_with(|| current.clone());
        ui.horizontal(|ui| {
            let response = ui
                .add(TextEdit::singleline(name).desired_width(280.0))
                .accessible_name("Workspace name");
            rename |= response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            let trimmed = name.trim();
            rename |= ui
                .add_enabled(
                    !trimmed.is_empty() && trimmed != current,
                    Button::new("Rename"),
                )
                .clicked();
        });
        muted(
            ui,
            "The workspace switcher shows this name. The folder keeps its own.",
        );
    });
    field(ui, "Location", Role::Group, |ui| {
        ui.add(egui::Label::new(&root).selectable(true).wrap());
        reveal = ui.button("Show folder").clicked();
    });
    if rename
        && let Some(name) = view.name.clone()
        && (app.rename_workspace(&name) || name.trim().is_empty())
    {
        // Show the name the workspace has now.
        view.name = None;
    }
    if reveal {
        app.reveal_workspace();
    }
}

fn about(ui: &mut Ui, app: &mut App) {
    let mut copy = false;
    let mut opened = None;
    field(ui, "Version", Role::Group, |ui| {
        ui.label(VERSION);
    });
    field(ui, "License", Role::Group, |ui| {
        ui.label(format!("{LICENSE}, free and open source"));
    });
    field(ui, "Diagnostics", Role::Group, |ui| {
        muted(
            ui,
            "Copy a report to attach to a bug report. Noura leaves out file names, paths and \
             quoted text.",
        );
        copy = ui.button("Copy diagnostics").clicked();
    });
    field(ui, "Links", Role::Group, |ui| {
        for link in &ABOUT_LINKS {
            if ui.link(link.title).on_hover_text(link.url).clicked() {
                opened = Some(link);
            }
        }
    });
    if copy {
        app.copy_diagnostics();
    }
    if let Some(link) = opened {
        app.open_about_link(link);
    }
}
