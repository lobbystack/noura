//! Settings' Plugins section, the install dialog and the first-run screen
//! for the plugins that ship with Noura.
//!
//! The section installs a plugin from a folder or from the address of a
//! release on GitHub, and lists each plugin installed on this device or
//! turned on in the workspace, with its version, where it came from, and
//! each capability, network host and command target you approved, with a
//! button to revoke it. A plugin from GitHub checks for a newer release
//! from its row. The install dialog lists every capability a plugin
//! declares before any of its code runs, and warns that nobody reviewed a
//! plugin from GitHub; its keyboard starts on Cancel.

use app_core::{App, CapabilityLine, Grant, PluginRow, UpdateCheck};
use egui::{
    Button, Frame, Id, Key, Margin, RichText, ScrollArea, Spinner, Stroke, TextEdit, Ui, UiBuilder,
    accesskit::Role,
};

/// What the install dialog says about a plugin from GitHub.
const UNREVIEWED: &str = "Nobody reviewed this plugin. Install it only if you trust its author.";

use crate::{
    FolderPicker,
    common::{Btn, Size, Variant, description},
    icons,
    theme::{BODY_SIZE, Palette, TITLE_SIZE, Weight, font, radius},
};

/// What the install dialog remembers between frames.
#[derive(Default)]
pub(crate) struct InstallView {
    /// The dialog showed last frame.
    open: bool,
    /// The dialog showed a release downloading last frame.
    downloading: bool,
}

/// What the legacy app said each plugin that ships with Noura does.
fn shipped_description(id: &str) -> Option<&'static str> {
    Some(match id {
        "ai" => "AI chat and provider settings",
        "tasks" => "Tasks and due dates",
        "projects" => "Project notes and task boards",
        "calendar" => "Tasks by date",
        _ => return None,
    })
}

/// The line under a plugin's name: what it does, or else its version.
fn plugin_summary(row: &PluginRow) -> String {
    let id = row.id.rsplit(':').next().unwrap_or(&row.id);
    match (
        row.shipped.then(|| shipped_description(id)).flatten(),
        &row.version,
    ) {
        (Some(text), _) => text.to_owned(),
        (None, Some(version)) => format!("Version {version}"),
        (None, None) => "Not installed on this computer".to_owned(),
    }
}

/// The icon a plugin shows: its page's, or a puzzle piece.
fn plugin_icon(row: &PluginRow) -> icons::Icon {
    match row.id.rsplit(':').next().unwrap_or(&row.id) {
        "ai" => icons::AI,
        "tasks" => icons::TASKS,
        "projects" => icons::PROJECTS,
        "calendar" => icons::CALENDAR,
        _ => icons::PUZZLE_PIECE,
    }
}

/// The Plugins section's content: the list of plugins with their
/// switches, or one plugin's details, then the ways to install another.
/// `address` holds what you typed in the release address field.
pub(crate) fn section(
    ui: &mut Ui,
    app: &mut App,
    picker: &mut dyn FolderPicker,
    address: &mut String,
) {
    let rows = app.plugins();
    let workspace_open = app.workspace().is_some();
    let details = app
        .settings()
        .and_then(|settings| settings.plugin_details.clone());
    if let Some(row) = details.and_then(|id| rows.iter().find(|row| row.id == id)) {
        details_page(ui, app, row, workspace_open);
        return;
    }
    if !workspace_open {
        description(ui, "Open a workspace first.");
        ui.add_space(16.0);
    }
    if rows.is_empty() {
        description(ui, "No plugins installed");
    }
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new().accessibility_role(Role::List);
    let mut open = None;
    ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                let y = ui.cursor().top();
                ui.painter()
                    .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, palette.border));
            }
            let builder = UiBuilder::new().accessibility_role(Role::ListItem);
            ui.push_id(("plugin-row", &row.id), |ui| {
                ui.scope_builder(builder, |ui| {
                    if list_row(ui, app, row, workspace_open) {
                        open = Some(row.id.clone());
                    }
                });
            });
        }
    });
    if let Some(id) = open {
        app.show_plugin_details(Some(&id));
    }
    ui.add_space(32.0);
    install_groups(ui, app, picker, address);
}

/// One plugin in the list: its icon, name and what it does, which open
/// its details, and its switch. Returns whether you opened the details.
fn list_row(ui: &mut Ui, app: &mut App, row: &PluginRow, workspace_open: bool) -> bool {
    let palette = Palette::of(ui.ctx());
    let mut enabled = row.enabled;
    let mut open = false;
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        let width = ui.available_width() - 32.0 - 16.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 44.0), egui::Sense::click());
        let label = format!("{} settings", row.name);
        response.widget_info(|| egui::WidgetInfo::labeled(Role::Button, true, &label));
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            let icon = egui::Rect::from_center_size(
                egui::pos2(rect.left() + 20.0, rect.center().y),
                egui::vec2(20.0, 20.0),
            );
            icons::paint(
                painter,
                plugin_icon(row),
                icon,
                palette.muted_foreground,
                false,
            );
            let name = painter.layout_no_wrap(
                row.name.clone(),
                font(ui.ctx(), BODY_SIZE, Weight::Medium),
                palette.foreground,
            );
            let summary = painter.layout_no_wrap(
                plugin_summary(row),
                egui::FontId::proportional(BODY_SIZE),
                palette.muted_foreground,
            );
            let left = rect.left() + 40.0 + 16.0;
            painter.galley(egui::pos2(left, rect.top()), name, palette.foreground);
            painter.galley(
                egui::pos2(left, rect.top() + 24.0),
                summary,
                palette.muted_foreground,
            );
            let caret = egui::Rect::from_center_size(
                egui::pos2(rect.right() - 8.0, rect.center().y),
                egui::vec2(16.0, 16.0),
            );
            icons::paint(
                painter,
                icons::CARET_RIGHT,
                caret,
                palette.muted_foreground,
                false,
            );
            if response.has_focus() {
                crate::common::focus_ring(painter, rect.expand(2.0), radius(1.0), &palette);
            }
        }
        open = response.clicked();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = format!("Enable {}", row.name);
            crate::common::switch(ui, &mut enabled, &label, workspace_open);
        });
    });
    ui.add_space(16.0);
    if enabled != row.enabled {
        app.set_plugin_enabled(&row.id, enabled);
    }
    open
}

/// One plugin's page: back to the list, its name with its switch, then
/// its version, source and what it may use.
fn details_page(ui: &mut Ui, app: &mut App, row: &PluginRow, workspace_open: bool) {
    let palette = Palette::of(ui.ctx());
    if Btn::new("Plugins")
        .icon(icons::BACK)
        .variant(Variant::Ghost)
        .size(Size::Sm)
        .show(ui)
        .clicked()
    {
        app.show_plugin_details(None);
        return;
    }
    ui.add_space(24.0);
    let mut enabled = row.enabled;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
        let icon = egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0));
        icons::paint(
            ui.painter(),
            plugin_icon(row),
            icon,
            palette.foreground,
            false,
        );
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 48.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(RichText::new(&row.name).font(font(ui.ctx(), TITLE_SIZE, Weight::Semibold)));
            description(ui, &plugin_summary(row));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let label = format!("Enable {}", row.name);
            crate::common::switch(ui, &mut enabled, &label, workspace_open);
        });
    });
    if enabled != row.enabled {
        app.set_plugin_enabled(&row.id, enabled);
    }
    ui.add_space(24.0);
    ui.separator();
    ui.add_space(24.0);
    plugin(ui, app, row);
}

/// Installing from a folder and from a release on GitHub.
fn install_groups(ui: &mut Ui, app: &mut App, picker: &mut dyn FolderPicker, address: &mut String) {
    let mut pick = false;
    group(ui, "Install from a folder", |ui| {
        description(
            ui,
            "A folder with a plugin.toml. Noura reloads each new build.",
        );
        pick = Btn::new("Install from folder\u{2026}").show(ui).clicked();
    });
    if pick && let Some(folder) = picker.pick_folder("Choose a plugin folder") {
        app.review_plugin_folder(folder);
    }
    let mut download = false;
    group(ui, "Install from GitHub", |ui| {
        description(ui, "Noura shows what it asks for before installing.");
        let typed = !address.trim().is_empty();
        ui.horizontal(|ui| {
            let button = Btn::new("Install from GitHub\u{2026}").enabled(typed);
            let field = ui
                .add(
                    TextEdit::singleline(address)
                        .hint_text("https://github.com/owner/plugin/releases/tag/v1.0.0")
                        .desired_width(button.room_before(ui)),
                )
                .accessible_name("Release address");
            download =
                typed && field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            download |= button.show(ui).clicked();
        });
    });
    if download {
        app.review_plugin_release(address);
    }
}

/// What you asked for in a plugin's row.
enum RowAction {
    Revoke(Grant),
    Allow(Grant),
    ApproveUpdate,
    DeclineUpdate,
    CheckForUpdates,
    Restart,
    Uninstall,
}

/// One plugin's details: where it came from, what it may use with a way
/// to revoke each, a waiting update, and Uninstall.
fn plugin(ui: &mut Ui, app: &mut App, row: &PluginRow) {
    let mut action = None;
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label(&row.name);
    ui.push_id(("plugin-row", &row.id), |ui| {
        ui.scope_builder(builder, |ui| {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                if let Some(version) = &row.version {
                    description(ui, &format!("Version {version}"));
                }
                if row.shipped {
                    description(ui, "Ships with Noura");
                } else if let Some(release) = &row.release {
                    description(ui, "From GitHub, not reviewed");
                    ui.add(egui::Label::new(release).selectable(true).wrap());
                } else if let Some(folder) = &row.folder {
                    ui.add(egui::Label::new(folder).selectable(true).wrap());
                }
                if !row.installed {
                    description(ui, "On in this workspace, but not installed here.");
                }
                if !row.approved.is_empty() {
                    ui.add_space(4.0);
                    ui.label("You allowed it to:");
                    if let Some(grant) = grant_lines(ui, &row.approved, "Revoke") {
                        action = Some(RowAction::Revoke(grant));
                    }
                }
                if !row.revoked.is_empty() {
                    ui.add_space(4.0);
                    ui.label("You revoked:");
                    if let Some(grant) = grant_lines(ui, &row.revoked, "Allow") {
                        action = Some(RowAction::Allow(grant));
                    }
                }
                if let Some(problem) = &row.problem {
                    alert(ui, problem);
                }
                if row.failed && ui.button("Start again").clicked() {
                    action = Some(RowAction::Restart);
                }
                if row.release.is_some() {
                    if let Some(update) = release_update(ui, row) {
                        action = Some(update);
                    }
                } else if !row.update.is_empty() {
                    ui.add_space(4.0);
                    ui.label("The new build also asks to:");
                    capabilities(ui, &row.update);
                    ui.horizontal(|ui| {
                        if ui.button("Allow and run").clicked() {
                            action = Some(RowAction::ApproveUpdate);
                        }
                        if ui.button("Keep current").clicked() {
                            action = Some(RowAction::DeclineUpdate);
                        }
                    });
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let checking = row.update_check == Some(UpdateCheck::Checking);
                    if row.installed
                        && row.release.is_some()
                        && ui
                            .add_enabled(!checking, Button::new("Check for updates"))
                            .clicked()
                    {
                        action = Some(RowAction::CheckForUpdates);
                    }
                    if row.installed && ui.button("Uninstall").clicked() {
                        action = Some(RowAction::Uninstall);
                    }
                });
            });
        });
    });
    match action {
        Some(RowAction::Revoke(grant)) => {
            app.revoke_plugin_grant(&row.id, &grant);
        }
        Some(RowAction::Allow(grant)) => {
            app.allow_plugin_grant(&row.id, &grant);
        }
        Some(RowAction::ApproveUpdate) => {
            app.approve_plugin_update(&row.id);
        }
        Some(RowAction::DeclineUpdate) => {
            app.decline_plugin_update(&row.id);
        }
        Some(RowAction::CheckForUpdates) => {
            app.check_plugin_update(&row.id);
        }
        Some(RowAction::Restart) => {
            app.restart_plugin(&row.id);
        }
        Some(RowAction::Uninstall) => {
            app.uninstall_plugin(&row.id);
        }
        None => {}
    }
}

/// Each line's description, its manifest name and a button named `verb`
/// that screen readers hear with the description. Returns the grant whose
/// button was pressed.
fn grant_lines(ui: &mut Ui, lines: &[CapabilityLine], verb: &str) -> Option<Grant> {
    let mut pressed = None;
    for line in lines {
        ui.push_id((&line.grant, verb), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("\u{2022}");
                ui.label(&line.description);
                description(ui, &line.name);
                let response = ui.small_button(verb);
                let label = format!("{verb}: {}", line.description);
                ui.ctx().accesskit_node_builder(response.id, |node| {
                    node.set_label(label);
                });
                if response.clicked() {
                    pressed = Some(line.grant.clone());
                }
            });
        });
    }
    pressed
}

/// Where checking GitHub for a newer release of `row`'s plugin stands, and
/// a newer release that waits, with only what it adds. Returns what you
/// pressed.
fn release_update(ui: &mut Ui, row: &PluginRow) -> Option<RowAction> {
    match &row.update_check {
        Some(UpdateCheck::Checking) => {
            ui.horizontal(|ui| {
                ui.add(Spinner::new());
                description(ui, "Checking for updates\u{2026}");
            });
        }
        Some(UpdateCheck::Newest(_)) => {
            description(ui, "Up to date");
        }
        Some(UpdateCheck::Failed(problem)) => alert(ui, problem),
        None => {}
    }
    let newer = row.newer.as_ref()?;
    let mut action = None;
    ui.add_space(4.0);
    ui.label(format!("Version {newer} is available."));
    let install = if row.update.is_empty() {
        format!("Install version {newer}")
    } else {
        ui.label("It also asks to:");
        capabilities(ui, &row.update);
        "Allow and install".to_owned()
    };
    ui.horizontal(|ui| {
        if ui.button(install).clicked() {
            action = Some(RowAction::ApproveUpdate);
        }
        let keep = match &row.version {
            Some(version) => format!("Keep version {version}"),
            None => "Keep current".to_owned(),
        };
        if ui.button(keep).clicked() {
            action = Some(RowAction::DeclineUpdate);
        }
    });
    action
}

/// The first-run screen: the plugins that ship with Noura and what each
/// may use, approved for this device with one click. None of their code
/// runs before.
pub(crate) fn shipped_dialog(ui: &mut Ui, app: &mut App, view: &mut ShippedView) {
    let plugins = app.shipped_plugins();
    if plugins.is_empty() {
        view.open = false;
        return;
    }
    let opened = !view.open;
    view.open = true;
    let mut approve = false;
    crate::common::dialog(ui.ctx(), Id::unique("shipped-plugins")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::Dialog)
            .accessibility_label("Plugins that ship with Noura");
        ui.scope_builder(builder, |ui| {
            ui.set_width(460.0);
            ui.label(
                RichText::new("Plugins that ship with Noura").font(crate::theme::font(
                    ui.ctx(),
                    TITLE_SIZE,
                    crate::theme::Weight::Medium,
                )),
            );
            description(
                ui,
                "Each runs with only what it lists. Revoke it in Settings > Plugins.",
            );
            ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for plugin in &plugins {
                    ui.add_space(8.0);
                    let builder = UiBuilder::new()
                        .accessibility_role(Role::Group)
                        .accessibility_label(&plugin.name);
                    ui.push_id(("shipped", &plugin.id), |ui| {
                        ui.scope_builder(builder, |ui| {
                            ui.label(RichText::new(&plugin.name).strong());
                            description(ui, &format!("Version {}", plugin.version));
                            if plugin.capabilities.is_empty() {
                                ui.label("It asks for no capabilities.");
                            } else {
                                capabilities(ui, &plugin.capabilities);
                            }
                        });
                    });
                }
            });
            let button = crate::common::dialog_footer(
                ui,
                vec![Btn::new("Allow and continue").variant(Variant::Default)],
            )
            .remove(0);
            if opened {
                button.request_focus();
            }
            approve = button.clicked();
        });
    });
    if approve {
        app.approve_shipped_plugins();
    }
}

/// What the first-run screen remembers between frames.
#[derive(Default)]
pub(crate) struct ShippedView {
    /// The screen showed last frame.
    open: bool,
}

/// The install dialog, while it is open. Escape or Cancel closes it
/// without installing anything.
pub(crate) fn install_dialog(ui: &mut Ui, app: &mut App, view: &mut InstallView) {
    let Some(install) = app.plugin_install().cloned() else {
        view.open = false;
        view.downloading = false;
        return;
    };
    let opened = !view.open;
    view.open = true;
    // The dialog's content changes once the download is in, so the
    // keyboard goes back to Cancel.
    let downloaded = view.downloading && !install.downloading;
    view.downloading = install.downloading;
    let mut cancel = false;
    let mut approve = false;
    let modal =
        crate::common::dialog(ui.ctx(), Id::unique("plugin-install")).show(ui.ctx(), |ui| {
            let builder = UiBuilder::new()
                .accessibility_role(Role::Dialog)
                .accessibility_label("Install plugin");
            ui.scope_builder(builder, |ui| {
                ui.set_width(420.0);
                let title = match &install.problem {
                    Some(_) => "Noura can't install this plugin".to_owned(),
                    None if install.downloading => "Downloading the plugin\u{2026}".to_owned(),
                    None => format!("Install {}?", install.name),
                };
                ui.label(RichText::new(title).font(crate::theme::font(
                    ui.ctx(),
                    TITLE_SIZE,
                    crate::theme::Weight::Medium,
                )));
                if install.problem.is_none() && !install.downloading {
                    description(ui, &format!("Version {}", install.version));
                }
                ui.add(egui::Label::new(&install.source).selectable(true).wrap());
                ui.add_space(8.0);
                match &install.problem {
                    Some(problem) => alert(ui, problem),
                    None if install.downloading => {
                        ui.add(Spinner::new());
                    }
                    None if install.from_github => {
                        warning(ui, UNREVIEWED);
                        ui.add_space(8.0);
                        capability_list(ui, &install.capabilities);
                    }
                    None => capability_list(ui, &install.capabilities),
                }
                let label = if install.problem.is_some() {
                    "Close"
                } else {
                    "Cancel"
                };
                let mut buttons = vec![Btn::new(label)];
                if install.problem.is_none() && !install.downloading {
                    buttons.push(Btn::new("Install").variant(Variant::Default));
                }
                let pressed = crate::common::dialog_footer(ui, buttons);
                if opened || downloaded {
                    pressed[0].request_focus();
                }
                cancel = pressed[0].clicked();
                approve = pressed.get(1).is_some_and(egui::Response::clicked);
            });
        });
    if approve {
        app.approve_plugin_install();
    } else if cancel || modal.should_close() {
        app.cancel_plugin_install();
    }
}

/// What a plugin asks for, and that it gets nothing more.
fn capability_list(ui: &mut Ui, lines: &[CapabilityLine]) {
    if lines.is_empty() {
        ui.label("It asks for no capabilities.");
        return;
    }
    ui.label("It asks to:");
    capabilities(ui, lines);
    ui.add_space(4.0);
    description(ui, "It gets only these capabilities.");
}

/// A caution in a frame of its own.
fn warning(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    Frame::new()
        .stroke(Stroke::new(1.0, palette.destructive))
        .corner_radius(radius(1.0))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.add(egui::Label::new(text).wrap());
        });
}

/// Each capability's description, with its manifest name beside it.
fn capabilities(ui: &mut Ui, lines: &[CapabilityLine]) {
    for line in lines {
        ui.horizontal_wrapped(|ui| {
            ui.label("\u{2022}");
            ui.label(&line.description);
            description(ui, &line.name);
        });
    }
}

/// A problem, read out by screen readers as it appears.
fn alert(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Alert)
        .accessibility_label(text);
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(text).color(palette.destructive));
    });
}

/// A titled group of controls.
fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    crate::common::settings_group(ui, title, add);
}
