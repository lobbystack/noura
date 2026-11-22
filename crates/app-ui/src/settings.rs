//! Settings: a large sheet with `app-core`'s sections under their groups
//! on the left, a search field above them, and the chosen section on the
//! right. Escape or the close button closes it.

use app_core::{
    ABOUT_LINKS, App, Appearance, LICENSE, LineWidth, McpTest, NoticeKind, Settings, TextSize,
    VERSION, settings_section,
};
use egui::{
    Color32, CornerRadius, Frame, Id, Margin, Modal, Rect, RichText, ScrollArea, Sense, Shadow,
    Stroke, TextEdit, Ui, UiBuilder, accesskit::Role, pos2, vec2,
};

use crate::{
    FolderPicker, common,
    common::{Btn, Size, Variant, description, field_label, muted},
    icons::{self, Icon},
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, Weight, font, radius},
};

/// The sheet's largest size; it keeps 24 px from each window edge.
const MAX_WIDTH: f32 = 1040.0;
const MAX_HEIGHT: f32 = 740.0;
/// The width of the section list.
const NAV_WIDTH: f32 = 220.0;
/// The widest a section's contents run.
const CONTENT_WIDTH: f32 = 680.0;

/// What Settings remembers between frames.
#[derive(Default)]
pub(crate) struct SettingsView {
    /// Settings was open last frame.
    open: bool,
    /// The widget that had the keyboard before Settings opened.
    return_focus: Option<Id>,
    /// What the Sync account section remembers.
    account: crate::account::AccountView,
    /// The AI providers section.
    ai: crate::ai::AiSettingsView,
    /// The Web search section.
    web: crate::web_search::WebSettingsView,
    /// The Speech section.
    speech: crate::speech::SpeechSettingsView,
    /// The release address being typed in Plugins.
    release_address: String,
    /// The Members section.
    members: crate::members::MembersView,
    /// The Ignored files section.
    ignored: crate::ignored_files::IgnoredFilesView,
}

/// The icon a section shows in the list: the kernel's sections have the
/// legacy app's, and a plugin's own page a puzzle piece.
fn section_icon(id: &str) -> Icon {
    match id {
        settings_section::GENERAL => icons::SETTINGS,
        settings_section::EDITOR => icons::TEXT_AA,
        settings_section::SHORTCUTS => icons::KEYBOARD,
        settings_section::SPEECH => icons::DESKTOP,
        settings_section::SYNC_ACCOUNT => icons::USER_CIRCLE,
        settings_section::WORKSPACE => icons::FOLDER_SIMPLE,
        settings_section::AI => icons::AI,
        settings_section::AI_TOOLS => icons::PLUGS_CONNECTED,
        settings_section::SYNC => icons::ARROWS_CLOCKWISE,
        settings_section::MEMBERS => icons::USERS,
        settings_section::IGNORED_FILES => icons::FILES,
        settings_section::ABOUT => icons::INFO,
        _ => icons::PUZZLE_PIECE,
    }
}

/// The line under a section's title, where the legacy app had one.
fn section_description(id: &str) -> Option<&'static str> {
    Some(match id {
        settings_section::IGNORED_FILES => "Patterns follow .gitignore.",
        _ => return None,
    })
}

/// Draw Settings while it is open in the window whose ID salt is
/// `window`. Once it closes, the keyboard goes back where it was, or to
/// the rail's Settings button when nothing had it.
pub(crate) fn show(
    ui: &mut Ui,
    app: &mut App,
    window: Id,
    view: &mut SettingsView,
    picker: &mut dyn FolderPicker,
    spelling: bool,
) {
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
    let palette = Palette::of(ui.ctx());
    let dark = ui.visuals().dark_mode;
    let mut close = false;
    let screen = ui.ctx().content_rect().size();
    let size = vec2(
        MAX_WIDTH.min(screen.x - 48.0).max(320.0),
        MAX_HEIGHT.min(screen.y - 48.0).max(240.0),
    );
    let frame = Frame::new()
        .fill(palette.popover)
        .corner_radius(CornerRadius::same(24))
        .stroke(Stroke::new(
            1.0,
            palette
                .foreground
                .gamma_multiply(if dark { 0.1 } else { 0.05 }),
        ))
        .shadow(Shadow {
            offset: [0, 20],
            blur: 25,
            spread: 0,
            color: Color32::from_black_alpha(26),
        });
    let modal = Modal::new(Id::unique("settings"))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(77))
        .show(ui.ctx(), |ui| {
            let builder = UiBuilder::new()
                .accessibility_role(Role::Dialog)
                .accessibility_label("Settings");
            ui.scope_builder(builder, |ui| {
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let nav_rect = Rect::from_min_size(rect.min, vec2(NAV_WIDTH, rect.height()));
                let content_rect = Rect::from_min_max(pos2(nav_rect.right(), rect.top()), rect.max);
                // The list's side of the sheet, with its own fill and a
                // border on its right.
                ui.painter().rect_filled(
                    nav_rect,
                    CornerRadius {
                        nw: 24,
                        sw: 24,
                        ne: 0,
                        se: 0,
                    },
                    palette.sidebar,
                );
                ui.painter().vline(
                    nav_rect.right(),
                    nav_rect.y_range(),
                    Stroke::new(1.0, palette.border),
                );
                let mut nav_ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(nav_rect.shrink(16.0))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                nav(&mut nav_ui, app, &settings, opened);
                let mut content_ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(content_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                ScrollArea::vertical()
                    .id_salt(("settings-content", &settings.section))
                    .auto_shrink(false)
                    .show(&mut content_ui, |ui| {
                        let width = (ui.available_width() - 80.0).min(CONTENT_WIDTH);
                        let side = ((ui.available_width() - width) / 2.0).max(40.0);
                        // The section's title shares the baseline of the
                        // sheet's own title across the sidebar.
                        Frame::new()
                            .inner_margin(Margin {
                                left: side as i8,
                                right: side as i8,
                                top: 24,
                                bottom: 40,
                            })
                            .show(ui, |ui| {
                                ui.set_width(width);
                                content(ui, app, window, &settings, view, picker, spelling);
                            });
                    });
                // The close button sits over the sheet's top right corner.
                let close_rect = Rect::from_min_size(
                    pos2(rect.right() - 16.0 - 28.0, rect.top() + 16.0),
                    vec2(28.0, 28.0),
                );
                let mut close_ui = ui.new_child(UiBuilder::new().max_rect(close_rect));
                close = Btn::icon_only(icons::CLOSE, "Close")
                    .size(Size::Sm)
                    .fill(palette.secondary)
                    .show(&mut close_ui)
                    .clicked();
            });
        });
    if close || modal.should_close() {
        app.close_settings();
    }
}

/// The sheet's title, Search settings and the section list: each group's
/// title, then its sections.
fn nav(ui: &mut Ui, app: &mut App, settings: &Settings, opened: bool) {
    let palette = Palette::of(ui.ctx());
    let width = ui.available_width();
    ui.spacing_mut().item_spacing.y = 0.0;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.set_height(24.0);
        ui.add_space(8.0);
        ui.label(RichText::new("Settings").font(font(ui.ctx(), BODY_SIZE, Weight::Semibold)));
    });
    ui.add_space(16.0);
    let mut query = settings.query.clone();
    let search = ui
        .add(
            TextEdit::singleline(&mut query)
                .hint_text("Search settings\u{2026}")
                .desired_width(width),
        )
        .accessible_name("Search settings");
    if search.changed() {
        app.set_settings_query(&query);
    }
    ui.add_space(20.0);
    let mut chosen = None;
    let builder = UiBuilder::new()
        .accessibility_role(Role::Navigation)
        .accessibility_label("Settings sections");
    ScrollArea::vertical()
        .id_salt("settings-sections")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.scope_builder(builder, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let groups = app.matching_settings_sections();
                if groups.is_empty() {
                    let builder = UiBuilder::new().accessibility_role(Role::Status);
                    ui.scope_builder(builder, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("No settings found.")
                                    .size(BODY_SIZE)
                                    .color(palette.muted_foreground),
                            );
                        });
                    });
                }
                for (index, (group, sections)) in groups.into_iter().enumerate() {
                    if index > 0 {
                        ui.add_space(24.0);
                    }
                    let builder = UiBuilder::new()
                        .accessibility_role(Role::Group)
                        .accessibility_label(group.title());
                    ui.scope_builder(builder, |ui| {
                        ui.horizontal(|ui| {
                            ui.set_height(16.0);
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(group.title())
                                    .size(SUPPORTING_SIZE)
                                    .color(palette.muted_foreground),
                            );
                        });
                        ui.add_space(8.0);
                        for (index, section) in sections.into_iter().enumerate() {
                            if index > 0 {
                                ui.add_space(4.0);
                            }
                            let selected = section.id == settings.section;
                            let variant = if selected {
                                Variant::Secondary
                            } else {
                                Variant::Ghost
                            };
                            let response = Btn::new(&section.title)
                                .icon(section_icon(&section.id))
                                .variant(variant)
                                .current(selected)
                                .width(width)
                                .show(ui);
                            if selected && opened {
                                response.request_focus();
                            }
                            if response.clicked() {
                                chosen = Some(section.id.clone());
                            }
                        }
                    });
                }
            });
        });
    if let Some(id) = chosen {
        app.select_settings_section(&id);
    }
}

/// The chosen section's title, its description, its notice and its
/// controls.
fn content(
    ui: &mut Ui,
    app: &mut App,
    window: Id,
    settings: &Settings,
    view: &mut SettingsView,
    picker: &mut dyn FolderPicker,
    spelling: bool,
) {
    let title = app
        .settings_sections()
        .into_iter()
        .flat_map(|(_, sections)| sections)
        .find(|section| section.id == settings.section)
        .map(|section| section.title.clone())
        .unwrap_or_default();
    ui.spacing_mut().item_spacing.y = 8.0;
    ui.label(crate::theme::text(
        ui.ctx(),
        title,
        crate::theme::LARGE_TITLE_SIZE,
        Weight::Semibold,
    ));
    if let Some(text) = section_description(&settings.section) {
        description(ui, text);
    }
    ui.add_space(24.0);
    notice(ui, settings);
    if settings.section != settings_section::MEMBERS {
        view.members.refreshed = false;
    }
    match settings.section.as_str() {
        settings_section::GENERAL => general(ui, app, settings),
        settings_section::EDITOR => editor(ui, app, spelling),
        settings_section::SHORTCUTS => crate::shortcuts::page(ui, app),
        settings_section::SPEECH => crate::speech::settings(ui, app, &mut view.speech),
        settings_section::WORKSPACE => workspace(ui, app),
        settings_section::PLUGINS => {
            crate::plugins::section(ui, app, picker, &mut view.release_address);
        }
        settings_section::SYNC_ACCOUNT => {
            crate::account::section(ui, app, &mut view.account, picker)
        }
        settings_section::SYNC => crate::folder_sync::section(ui, app),
        settings_section::MEMBERS => crate::members::section(ui, app, &mut view.members),
        settings_section::AI => {
            crate::ai::settings(ui, app, &mut view.ai);
            ui.add_space(24.0);
            crate::web_search::settings(ui, app, &mut view.web);
        }
        settings_section::IGNORED_FILES => ignored_files(ui, app, view),
        settings_section::ABOUT => about(ui, app),
        settings_section::AI_TOOLS => ai_tools(ui, app),
        id if app.is_plugin_settings(id) => crate::plugin_view::show_settings(ui, app, window, id),
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
            .corner_radius(radius(1.8))
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(&notice.title)
                        .font(font(ui.ctx(), BODY_SIZE, Weight::Medium))
                        .color(color),
                );
                if !notice.message.is_empty() {
                    description(ui, &notice.message);
                }
            });
    });
    ui.add_space(16.0);
}

/// A titled group of controls, such as Appearance's three choices: its
/// label, then its controls 12 px below, then 24 px to the next field.
fn field(ui: &mut Ui, title: &str, role: Role, add: impl FnOnce(&mut Ui)) {
    let builder = UiBuilder::new()
        .accessibility_role(role)
        .accessibility_label(title);
    ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        field_label(ui, title);
        add(ui);
    });
    ui.add_space(24.0);
}

/// A field with its label and description on the left and a switch on
/// the right, such as Open at login.
fn switch_field(
    ui: &mut Ui,
    title: &str,
    on: &mut bool,
    enabled: bool,
    problem: Option<&str>,
) -> bool {
    let mut changed = false;
    ui.horizontal_top(|ui| {
        let width = ui.available_width() - 32.0 - 16.0;
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 4.0;
            field_label(ui, title);
            if let Some(problem) = problem {
                let palette = Palette::of(ui.ctx());
                let builder = UiBuilder::new().accessibility_role(Role::Alert);
                ui.scope_builder(builder, |ui| {
                    ui.label(RichText::new(problem).color(palette.destructive));
                });
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            changed = common::switch(ui, on, title, enabled).changed();
        });
    });
    ui.add_space(24.0);
    changed
}

fn general(ui: &mut Ui, app: &mut App, settings: &Settings) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("Updates");
    ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        field_label(ui, "Updates");
        crate::updates::update_settings(ui, app);
    });
    ui.add_space(24.0);

    let preferences = *app.preferences();
    let mut appearance = preferences.appearance;
    field(ui, "Appearance", Role::RadioGroup, |ui| {
        let options: Vec<_> = Appearance::ALL
            .into_iter()
            .map(|choice| {
                let icon = match choice {
                    Appearance::Light => icons::SUN,
                    Appearance::Dark => icons::MOON,
                    Appearance::MatchSystem => icons::DESKTOP,
                };
                (choice, choice.title(), Some(icon))
            })
            .collect();
        common::segmented_with_icons(ui, &options, &mut appearance);
    });
    if appearance != preferences.appearance {
        app.set_appearance(appearance);
    }

    let login = &settings.open_at_login;
    let mut enabled = login.enabled.unwrap_or(false);
    if switch_field(
        ui,
        "Open at login",
        &mut enabled,
        login.enabled.is_some(),
        login.problem.as_deref(),
    ) {
        app.set_open_at_login(enabled);
    }
}

/// The Editor section. `spelling` says whether the system has a spell
/// checker to turn on.
fn editor(ui: &mut Ui, app: &mut App, spelling: bool) {
    let current = *app.preferences();
    let mut text_size = current.text_size;
    field(ui, "Text size", Role::RadioGroup, |ui| {
        let options: Vec<_> = TextSize::ALL
            .into_iter()
            .map(|choice| (choice, choice.title()))
            .collect();
        common::segmented(ui, &options, &mut text_size);
    });
    if text_size != current.text_size {
        app.set_text_size(text_size);
    }

    let mut line_width = current.line_width;
    field(ui, "Line width", Role::RadioGroup, |ui| {
        let options: Vec<_> = LineWidth::ALL
            .into_iter()
            .map(|choice| (choice, choice.title()))
            .collect();
        common::segmented(ui, &options, &mut line_width);
    });
    if line_width != current.line_width {
        app.set_line_width(line_width);
    }

    if spelling {
        let mut spell_check = current.spell_check;
        if switch_field(ui, "Check spelling", &mut spell_check, true, None) {
            app.set_spell_check(spell_check);
        }
    }
}

/// The legacy empty state for a section that needs an open workspace.
fn no_workspace(ui: &mut Ui) {
    common::empty_state(ui, None, "No workspace open", None);
}

/// The Ignored files section: the page `app-core` opened for it.
fn ignored_files(ui: &mut Ui, app: &mut App, view: &mut SettingsView) {
    if app.workspace().is_none() {
        no_workspace(ui);
        return;
    }
    crate::ignored_files::page(ui, app, &mut view.ignored);
}

/// The Location section: the workspace folder's path and Show folder. The
/// section's title names it, so the path has no label of its own.
fn workspace(ui: &mut Ui, app: &mut App) {
    let Some(workspace) = app.workspace() else {
        no_workspace(ui);
        return;
    };
    let root = workspace.root().to_owned();
    let palette = Palette::of(ui.ctx());
    ui.spacing_mut().item_spacing.y = 12.0;
    ui.add(
        egui::Label::new(
            RichText::new(&root)
                .size(BODY_SIZE)
                .color(palette.muted_foreground),
        )
        .selectable(true)
        .wrap(),
    );
    let reveal = Btn::new("Show folder")
        .icon(icons::FOLDER_OPEN)
        .show(ui)
        .clicked();
    if reveal {
        app.reveal_workspace();
    }
}

/// The External tools section: what an assistant can do, a warning, the
/// MCP configuration for the open workspace with Copy, and Test
/// connection with its result.
fn ai_tools(ui: &mut Ui, app: &mut App) {
    let Some(setup) = app.mcp_setup() else {
        no_workspace(ui);
        return;
    };
    let palette = Palette::of(ui.ctx());
    common::alert(
        ui,
        icons::WARNING,
        "Assistants edit files directly",
        "Noura doesn\u{2019}t ask first. Connect only assistants you trust.",
        false,
    );
    ui.add_space(24.0);
    let mut copy = false;
    let mut test = false;
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("Configuration");
    ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() - 90.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                field_label(ui, "Configuration");
                description(
                    ui,
                    "Paste into your assistant\u{2019}s MCP settings, then restart it.",
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                copy = Btn::new("Copy")
                    .icon(icons::COPY)
                    .size(Size::Sm)
                    .enabled(setup.configuration.is_ok())
                    .show(ui)
                    .clicked();
            });
        });
        match &setup.configuration {
            Ok(configuration) => {
                Frame::new()
                    .fill(palette.muted)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(radius(1.0))
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                RichText::new(configuration)
                                    .monospace()
                                    .size(SUPPORTING_SIZE),
                            )
                            .selectable(true)
                            .wrap(),
                        );
                    });
            }
            Err(problem) => {
                let builder = UiBuilder::new().accessibility_role(Role::Alert);
                ui.scope_builder(builder, |ui| {
                    ui.label(RichText::new(problem).color(palette.destructive));
                });
            }
        }
        muted(
            ui,
            "If you move this workspace or Noura, copy the configuration again.",
        );
    });
    ui.add_space(24.0);
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label("Check the setup");
    ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        field_label(ui, "Check the setup");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let testing = setup.test == McpTest::Testing;
            let configured = setup.configuration.is_ok();
            test = Btn::new("Test connection")
                .enabled(configured && !testing)
                .show(ui)
                .clicked();
            let (role, text, color) = match &setup.test {
                McpTest::NotRun => return,
                McpTest::Testing => (
                    Role::Status,
                    "Testing\u{2026}".to_owned(),
                    palette.muted_foreground,
                ),
                McpTest::Works(server) => (
                    Role::Status,
                    format!("Connected to {server}."),
                    palette.foreground,
                ),
                McpTest::Failed(reason) => (
                    Role::Alert,
                    format!("Connection failed. {reason}"),
                    palette.destructive,
                ),
            };
            let builder = UiBuilder::new().accessibility_role(role);
            ui.scope_builder(builder, |ui| {
                ui.label(RichText::new(text).size(BODY_SIZE).color(color));
            });
        });
    });
    if copy {
        app.copy_mcp_configuration();
    }
    if test {
        app.test_mcp_connection();
    }
}

/// One row of the About list: a label on the left and its value on the
/// right, with a divider under it.
fn about_row(ui: &mut Ui, label: &str, last: bool, value: impl FnOnce(&mut Ui)) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label(label);
    ui.scope_builder(builder, |ui| {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 20.0;
            ui.label(RichText::new(label).size(BODY_SIZE));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), value);
        });
        ui.add_space(12.0);
    });
    if last {
        return;
    }
    let y = ui.cursor().top() - ui.spacing().item_spacing.y / 2.0;
    ui.painter()
        .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, palette.border));
}

fn about(ui: &mut Ui, app: &mut App) {
    let palette = Palette::of(ui.ctx());
    let mut copy = false;
    let mut opened = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    let value = |text: String| {
        move |ui: &mut Ui| {
            ui.label(
                RichText::new(text)
                    .size(BODY_SIZE)
                    .color(palette.muted_foreground),
            );
        }
    };
    about_row(ui, "Version", false, value(VERSION.to_owned()));
    about_row(ui, "License", false, value(LICENSE.to_owned()));
    about_row(ui, "Diagnostics", true, |ui| {
        copy = Btn::new("Copy diagnostics")
            .size(Size::Sm)
            .show(ui)
            .clicked();
    });
    ui.add_space(32.0);
    ui.spacing_mut().item_spacing.y = 10.0;
    let builder = UiBuilder::new()
        .accessibility_role(Role::List)
        .accessibility_label("Links");
    ui.scope_builder(builder, |ui| {
        for link in &ABOUT_LINKS {
            if common::link_item(ui, link.title, link.url).clicked() {
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
