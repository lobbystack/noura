//! Settings' Plugins section and the install dialog.
//!
//! The section lists each plugin installed on this device or turned on in
//! the workspace, with its version, its folder and the capabilities you
//! approved. The install dialog lists every capability a plugin folder
//! declares before any of its code runs; its keyboard starts on Cancel.

use app_core::{App, CapabilityLine, PluginRow};
use egui::{
    Button, Checkbox, Frame, Id, Margin, Modal, RichText, Stroke, Ui, UiBuilder, accesskit::Role,
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
        muted(ui, "No plugins are installed.");
    }
    let workspace_open = app.workspace().is_some();
    for row in rows {
        ui.add_space(8.0);
        plugin(ui, app, &row, workspace_open);
    }
}

/// One plugin's row: what it is, where it came from, what you approved,
/// and its switch.
fn plugin(ui: &mut Ui, app: &mut App, row: &PluginRow, workspace_open: bool) {
    let palette = Palette::of(ui.ctx());
    let mut enabled = row.enabled;
    let mut uninstall = false;
    let mut approve = false;
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
                    if let Some(folder) = &row.folder {
                        ui.add(egui::Label::new(folder).selectable(true).wrap());
                    }
                    if !row.installed {
                        muted(
                            ui,
                            "This workspace turns it on, but it isn't installed on this \
                             computer, so it doesn't run here.",
                        );
                    }
                    if !row.approved.is_empty() {
                        ui.add_space(4.0);
                        ui.label("You allowed it to:");
                        capabilities(ui, &row.approved);
                    }
                    if let Some(problem) = &row.problem {
                        alert(ui, problem);
                    }
                    if !row.update.is_empty() {
                        ui.add_space(4.0);
                        ui.label("A new build is waiting. It also asks to:");
                        capabilities(ui, &row.update);
                        approve = ui.button("Allow and run the new build").clicked();
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if workspace_open {
                            ui.add(Checkbox::new(&mut enabled, "On in this workspace"));
                        }
                        if row.installed {
                            uninstall = ui.button("Uninstall").clicked();
                        }
                    });
                });
        });
    });
    if enabled != row.enabled {
        app.set_plugin_enabled(&row.id, enabled);
    }
    if approve {
        app.approve_plugin_update(&row.id);
    }
    if uninstall {
        app.uninstall_plugin(&row.id);
    }
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
                        "None of its code runs until you install it. It gets only what it \
                         asks for here.",
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
