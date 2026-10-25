//! The Sync section of Settings (docs/workspace-format/sync-v2.md §15,
//! §16): whether the open workspace syncs, whether sync runs or you paused
//! it, the changes waiting to upload, the conflicts waiting for you, the
//! last error and the last successful sync, with Pause and Resume. It draws
//! what `app-core`'s folder sync screen holds and hands each choice to it.

use app_core::App;
use app_core::folder_sync::{FolderSyncAction, FolderSyncPhase};
use egui::{RichText, Ui, UiBuilder, accesskit::Role};

use crate::common::{muted, primary_button};
use crate::theme::Palette;

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

fn alert(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Alert)
        .accessibility_label(text);
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(text).color(palette.destructive));
    });
}

/// How many changes wait, in words.
pub(crate) fn pending_text(pending: usize) -> String {
    match pending {
        0 => "Every change is uploaded.".to_owned(),
        1 => "1 change waits to upload.".to_owned(),
        n => format!("{n} changes wait to upload."),
    }
}

/// Draws the Sync section.
pub(crate) fn section(ui: &mut Ui, app: &mut App) {
    let screen = app.folder_sync();
    let mut actions = Vec::new();
    if let Some(error) = &screen.last_error {
        alert(ui, error);
    }
    if screen.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            muted(ui, "Working on it.");
        });
    }
    match screen.phase {
        FolderSyncPhase::Unavailable => {
            ui.label(
                "Sign in to a sync account in Settings, then Sync account, to sync this workspace with your other computers.",
            );
        }
        FolderSyncPhase::Off => {
            ui.label("This workspace doesn't sync. Its files stay on this computer.");
            if primary_button(ui, "Turn on sync", !screen.busy).clicked() {
                actions.push(FolderSyncAction::TurnOn);
            }
        }
        FolderSyncPhase::On | FolderSyncPhase::Paused => {
            let paused = screen.phase == FolderSyncPhase::Paused;
            group(ui, "Status", |ui| {
                ui.label(if paused {
                    "Sync is paused. Your changes wait on this computer until you resume."
                } else {
                    "Sync is on."
                });
                ui.label(pending_text(screen.pending));
                match &screen.last_success {
                    Some(when) => ui.label(format!("Last synced {when}.")),
                    None => muted(ui, "This computer hasn't finished a sync yet."),
                };
                ui.horizontal(|ui| {
                    if paused {
                        if ui.button("Resume sync").clicked() {
                            actions.push(FolderSyncAction::Resume);
                        }
                    } else if ui.button("Pause sync").clicked() {
                        actions.push(FolderSyncAction::Pause);
                    }
                    if ui.button("Turn off sync").clicked() {
                        actions.push(FolderSyncAction::TurnOff);
                    }
                });
            });
            let title = match screen.conflicts.len() {
                0 => "No conflicts".to_owned(),
                1 => "1 conflict".to_owned(),
                n => format!("{n} conflicts"),
            };
            group(ui, &title, |ui| {
                if screen.conflicts.is_empty() {
                    muted(ui, "No file has versions from two computers.");
                }
                for conflict in &screen.conflicts {
                    ui.label(format!(
                        "{} has versions from two computers. Choose the one to keep:",
                        conflict.path
                    ));
                    ui.horizontal_wrapped(|ui| {
                        for branch in &conflict.branches {
                            if ui.button(&branch.label).clicked() {
                                actions.push(FolderSyncAction::Choose {
                                    rid: conflict.rid,
                                    key: branch.key,
                                });
                            }
                        }
                    });
                }
            });
            if !screen.skipped.is_empty() {
                group(ui, "Files sync leaves out", |ui| {
                    for line in &screen.skipped {
                        ui.label(line);
                    }
                });
            }
        }
    }
    for action in actions {
        app.folder_sync_action(action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pending_count_reads_as_words() {
        assert_eq!(pending_text(0), "Every change is uploaded.");
        assert_eq!(pending_text(1), "1 change waits to upload.");
        assert_eq!(pending_text(3), "3 changes wait to upload.");
    }
}
