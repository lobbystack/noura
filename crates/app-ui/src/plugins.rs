//! Settings' Plugins section, the install dialog and the first-run screen
//! for the plugins that ship with Noura.
//!
//! The section lists each plugin installed on this device or turned on in
//! the workspace, with its version, where it came from, and each
//! capability, network host and command target you approved, with a
//! button to revoke it. The install dialog lists every capability a plugin
//! folder declares before any of its code runs; its keyboard starts on
//! Cancel.

use app_core::{App, CapabilityLine, Grant, PluginRow};
use egui::{
    Button, Checkbox, Frame, Id, Margin, Modal, RichText, ScrollArea, Stroke, Ui, UiBuilder,
    accesskit::Role,
};

use crate::{
    FolderPicker,
    common::{muted, primary_button},
    theme::{Palette, TITLE_SIZE, radius},
};

/// What the install dialog remembers between frames.
#[derive(Default)]
pub(crate) struct InstallView {
    /// The dialog showed last frame.
    open: bool,
}

/// The Plugins section's content.
pub(crate) fn section(ui: &mut Ui, app: &mut App, picker: &mut dyn FolderPicker) {
    let mut pick = false;
    group(ui, "Install from a folder", |ui| {
        muted(
            ui,
            "Choose a folder with a plugin.toml and the plugin's component. Noura reloads the \
             plugin when you save a new build there.",
        );
        pick = ui.button("Install from folder\u{2026}").clicked();
    });
    if pick && let Some(folder) = picker.pick_folder("Choose a plugin folder") {
        app.review_plugin_folder(folder);
    }
    let rows = app.plugins();
    if rows.is_empty() {
        ui.add_space(8.0);
        muted(ui, "You haven't installed any plugins.");
    }
    let workspace_open = app.workspace().is_some();
    for row in rows {
        ui.add_space(8.0);
        plugin(ui, app, &row, workspace_open);
    }
}

/// What you asked for in a plugin's row.
enum RowAction {
    Revoke(Grant),
    Allow(Grant),
    ApproveUpdate,
    DeclineUpdate,
    Restart,
    Uninstall,
}

/// One plugin's row: what it is, where it came from, what it may use with
/// a way to revoke each, and its switch.
fn plugin(ui: &mut Ui, app: &mut App, row: &PluginRow, workspace_open: bool) {
    let palette = Palette::of(ui.ctx());
    let mut enabled = row.enabled;
    let mut action = None;
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label(&row.name);
    ui.push_id(("plugin-row", &row.id), |ui| {
        ui.scope_builder(builder, |ui| {
            Frame::new()
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(radius(1.0))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(&row.name).strong());
                    if let Some(version) = &row.version {
                        muted(ui, &format!("Version {version}"));
                    }
                    if row.shipped {
                        muted(ui, "Ships with Noura");
                    } else if let Some(folder) = &row.folder {
                        ui.add(egui::Label::new(folder).selectable(true).wrap());
                    }
                    if !row.installed {
                        muted(
                            ui,
                            "This workspace turns it on. Install it on this computer to run \
                             it here.",
                        );
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
                    if !row.update.is_empty() {
                        ui.add_space(4.0);
                        ui.label("A new build is waiting. It also asks to:");
                        capabilities(ui, &row.update);
                        ui.horizontal(|ui| {
                            if ui.button("Allow and run the new build").clicked() {
                                action = Some(RowAction::ApproveUpdate);
                            }
                            if ui.button("Keep the installed build").clicked() {
                                action = Some(RowAction::DeclineUpdate);
                            }
                        });
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if workspace_open {
                            ui.add(Checkbox::new(&mut enabled, "On in this workspace"));
                        }
                        if row.installed && ui.button("Uninstall").clicked() {
                            action = Some(RowAction::Uninstall);
                        }
                    });
                });
        });
    });
    if enabled != row.enabled {
        app.set_plugin_enabled(&row.id, enabled);
    }
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
                muted(ui, &line.name);
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
    Modal::new(Id::unique("shipped-plugins")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::Dialog)
            .accessibility_label("Plugins that ship with Noura");
        ui.scope_builder(builder, |ui| {
            ui.set_width(460.0);
            ui.label(
                RichText::new("Plugins that ship with Noura")
                    .size(TITLE_SIZE)
                    .strong(),
            );
            muted(
                ui,
                "Noura ships these plugins. They run on this computer once you allow them, \
                 each with only what it lists here. To take something back later, open \
                 Settings, then Plugins.",
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
                            muted(ui, &format!("Version {}", plugin.version));
                            if plugin.capabilities.is_empty() {
                                ui.label("It asks for no capabilities.");
                            } else {
                                capabilities(ui, &plugin.capabilities);
                            }
                        });
                    });
                }
            });
            ui.add_space(12.0);
            let button = primary_button(ui, "Allow and continue", true);
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
        return;
    };
    let opened = !view.open;
    view.open = true;
    let mut cancel = false;
    let mut approve = false;
    let modal = Modal::new(Id::unique("plugin-install")).show(ui.ctx(), |ui| {
        let builder = UiBuilder::new()
            .accessibility_role(Role::Dialog)
            .accessibility_label("Install plugin");
        ui.scope_builder(builder, |ui| {
            ui.set_width(420.0);
            let title = match &install.problem {
                Some(_) => "Noura can't install this plugin".to_owned(),
                None => format!("Install {}?", install.name),
            };
            ui.label(RichText::new(title).size(TITLE_SIZE).strong());
            if install.problem.is_none() {
                muted(ui, &format!("Version {}", install.version));
            }
            ui.add(egui::Label::new(&install.folder).selectable(true).wrap());
            ui.add_space(8.0);
            match &install.problem {
                Some(problem) => alert(ui, problem),
                None if install.capabilities.is_empty() => {
                    ui.label("It asks for no capabilities.");
                }
                None => {
                    ui.label("It asks to:");
                    capabilities(ui, &install.capabilities);
                    ui.add_space(4.0);
                    muted(
                        ui,
                        "Noura runs none of its code until you install it, and gives it only \
                         these capabilities.",
                    );
                }
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let label = if install.problem.is_some() {
                    "Close"
                } else {
                    "Cancel"
                };
                let cancel_button = ui.add(Button::new(label));
                if opened {
                    cancel_button.request_focus();
                }
                cancel = cancel_button.clicked();
                if install.problem.is_none() {
                    approve = primary_button(ui, "Install", true).clicked();
                }
            });
        });
    });
    if approve {
        app.approve_plugin_install();
    } else if cancel || modal.should_close() {
        app.cancel_plugin_install();
    }
}

/// Each capability's description, with its manifest name beside it.
fn capabilities(ui: &mut Ui, lines: &[CapabilityLine]) {
    for line in lines {
        ui.horizontal_wrapped(|ui| {
            ui.label("\u{2022}");
            ui.label(&line.description);
            muted(ui, &line.name);
        });
    }
}

/// A problem, read out by screen readers as it appears.
fn alert(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new().accessibility_role(Role::Alert);
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(text).color(palette.destructive));
    });
}

/// A titled group of controls.
fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label(title);
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(title).strong());
        add(ui);
    });
}
