//! The update section that Settings hosts, and the Restart offer every
//! screen shows once an update is ready.

use egui::{
    Align2, Area, Frame, Id, Margin, Order, RichText, Spinner, Stroke, Ui, UiBuilder, Widget,
    accesskit::Role,
};

use app_core::{App, UpdateStatus};

use crate::{
    common::{muted, primary_button},
    theme::{Palette, radius},
};

/// The Updates section: the running version, Check for updates and where
/// updating stands, or Restart to update once a new version is ready.
pub fn update_settings(ui: &mut Ui, app: &mut App) {
    let updates = app.updates();
    let version = updates.current_version().to_owned();
    let status = updates.status().clone();
    let busy = updates.is_busy();
    let palette = Palette::of(ui.ctx());

    ui.label(RichText::new("Updates").strong());
    if let UpdateStatus::Unavailable { reason } = &status {
        muted(ui, &format!("You have version {version}. {reason}"));
        return;
    }
    muted(
        ui,
        &format!(
            "You have version {version}. Noura downloads new versions in the background, and \
             you choose when to restart."
        ),
    );

    if let UpdateStatus::Ready {
        version: ready,
        notes,
        problem,
    } = &status
    {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("Version {ready} is ready")).strong());
                if let Some(notes) = notes {
                    muted(ui, notes);
                }
                if let Some(problem) = problem {
                    alert(ui, problem, &palette);
                }
                if primary_button(ui, "Restart to update", !busy).clicked() {
                    app.restart_to_update();
                }
            });
        return;
    }

    ui.horizontal(|ui| {
        let working = busy
            || matches!(
                status,
                UpdateStatus::Installing { .. } | UpdateStatus::Restarting { .. }
            );
        if ui
            .add_enabled(!working, egui::Button::new("Check for updates"))
            .clicked()
        {
            app.check_for_updates();
        }
        if working {
            Spinner::new().ui(ui).accessible_name("Updating");
        }
        let text = match &status {
            UpdateStatus::Checking => Some("Checking for updates…".to_owned()),
            UpdateStatus::UpToDate => Some("Noura is up to date.".to_owned()),
            UpdateStatus::Downloading { version } => {
                Some(format!("Downloading version {version}…"))
            }
            UpdateStatus::Installing { version } => Some(format!(
                "Saving your changes, then installing version {version}…"
            )),
            UpdateStatus::Restarting { .. } => Some("Restarting…".to_owned()),
            _ => None,
        };
        let builder = UiBuilder::new().accessibility_role(Role::Status);
        ui.scope_builder(builder, |ui| {
            if let Some(text) = text {
                muted(ui, &text);
            }
        });
    });
    if let UpdateStatus::Failed { message } = &status {
        alert(ui, message, &palette);
    }
}

/// Offer Restart in the window's corner while an update waits, until the
/// person chooses Later.
pub(crate) fn restart_offer(ui: &mut Ui, app: &mut App) {
    if !app.updates().offers_restart() {
        return;
    }
    let UpdateStatus::Ready {
        version, problem, ..
    } = app.updates().status().clone()
    else {
        return;
    };
    let busy = app.updates().is_busy();
    let palette = Palette::of(ui.ctx());
    Area::new(Id::unique("update-offer"))
        .order(Order::Foreground)
        .anchor(Align2::RIGHT_BOTTOM, [-16.0, -16.0])
        .show(ui.ctx(), |ui| {
            let builder = UiBuilder::new().accessibility_role(Role::Status);
            ui.scope_builder(builder, |ui| {
                Frame::popup(ui.style())
                    .fill(palette.popover)
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_max_width(320.0);
                        ui.label(RichText::new(format!("Noura {version} is ready")).strong());
                        match &problem {
                            Some(problem) => alert(ui, problem, &palette),
                            None => {
                                muted(ui, "Restart to finish updating.");
                            }
                        }
                        ui.horizontal(|ui| {
                            if primary_button(ui, "Restart", !busy).clicked() {
                                app.restart_to_update();
                            }
                            if ui.button("Later").clicked() {
                                app.hide_update_offer();
                            }
                        });
                    });
            });
        });
}

fn alert(ui: &mut Ui, text: &str, palette: &Palette) {
    let builder = UiBuilder::new().accessibility_role(Role::Alert);
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(text).color(palette.destructive));
    });
}
