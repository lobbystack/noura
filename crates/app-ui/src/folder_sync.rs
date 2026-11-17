//! The Sync section of Settings (docs/workspace-format/sync-v2.md §15,
//! §16): whether the open workspace syncs, whether sync runs or you paused
//! it, the changes waiting to upload, the conflicts waiting for you, the
//! last error and the last successful sync, with Pause and Resume. It draws
//! what `app-core`'s folder sync screen holds and hands each choice to it.

use app_core::App;
use app_core::folder_sync::{FolderSyncAction, FolderSyncPhase, HeldChoice};
use egui::{RichText, Ui, UiBuilder, accesskit::Role};

use crate::common::{description, primary_button};
use crate::theme::Palette;

fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    crate::common::settings_group(ui, title, add);
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
        0 => "All changes uploaded".to_owned(),
        1 => "1 change to upload".to_owned(),
        n => format!("{n} changes to upload"),
    }
}

/// How many changes wait for space, in words.
pub(crate) fn waiting_text(waiting: usize) -> String {
    match waiting {
        1 => "1 change waits for space".to_owned(),
        n => format!("{n} changes wait for space"),
    }
}

/// How many people, in words.
fn people_text(people: usize) -> String {
    match people {
        1 => "1 person in this workspace".to_owned(),
        n => format!("{n} people in this workspace"),
    }
}

/// Set up sync again for an owner of a running workspace, behind a
/// confirmation: a member's device may be the only one that found a loss,
/// so the owner's own device can start over without a stop of its own
/// (§14).
fn set_up_again(ui: &mut Ui, busy: bool, actions: &mut Vec<FolderSyncAction>) {
    let id = ui.scope_id().with("set_up_again_confirm");
    let asked = ui.data(|data| data.get_temp::<bool>(id).unwrap_or(false));
    group(ui, "Start this workspace over", |ui| {
        ui.label(
            "If the server lost part of this workspace, start it over from this folder. You invite members again, and restricted folders, share links and history reset.",
        );
        if !asked {
            if primary_button(ui, "Set up sync again", !busy).clicked() {
                ui.data_mut(|data| data.insert_temp(id, true));
            }
            return;
        }
        alert(
            ui,
            "It closes on the server for every member. Their files stay on their computers.",
        );
        ui.horizontal(|ui| {
            if primary_button(ui, "Close it and set up again", !busy).clicked() {
                actions.push(FolderSyncAction::SetUpAgain);
                ui.data_mut(|data| data.remove_temp::<bool>(id));
            }
            if ui.button("Keep this workspace").clicked() {
                ui.data_mut(|data| data.remove_temp::<bool>(id));
            }
        });
    });
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
            description(ui, "Working…");
        });
    }
    if let Some(notice) = &screen.restore {
        alert(ui, &notice.line);
        if !notice.written_again.is_empty() {
            group(ui, "Files this computer wrote again", |ui| {
                for path in &notice.written_again {
                    ui.label(path);
                }
            });
        }
        if !notice.to_review.is_empty() {
            group(ui, "Files to review after the restore", |ui| {
                for path in &notice.to_review {
                    ui.label(path);
                }
            });
        }
        ui.horizontal(|ui| {
            if notice.reset_devices && primary_button(ui, "Reset devices", !screen.busy).clicked() {
                actions.push(FolderSyncAction::ResetDevices);
            }
            if notice.set_up_again
                && primary_button(ui, "Set up sync again", !screen.busy).clicked()
            {
                actions.push(FolderSyncAction::SetUpAgain);
            }
            if !notice.reset_devices && !notice.set_up_again && ui.button("Dismiss").clicked() {
                actions.push(FolderSyncAction::DismissRestore);
            }
        });
    }
    match screen.phase {
        FolderSyncPhase::Unavailable => {
            ui.label("Sync is end-to-end encrypted. Sign in to turn it on.");
        }
        FolderSyncPhase::Unreachable => {
            ui.label("Can't reach the sync server. Noura keeps trying.");
        }
        FolderSyncPhase::Off => {
            ui.label("This workspace doesn't sync.");
            if !screen.on_account.is_empty() {
                group(ui, "Download a workspace here", |ui| {
                    for (workspace, name) in &screen.on_account {
                        let button = format!("Download {name}");
                        if primary_button(ui, &button, !screen.busy).clicked() {
                            actions.push(FolderSyncAction::DownloadHere(*workspace));
                        }
                    }
                });
            }
            let turn_on = if screen.on_account.is_empty() {
                "Turn on sync"
            } else {
                "Start a new workspace"
            };
            if primary_button(ui, turn_on, !screen.busy).clicked() {
                actions.push(FolderSyncAction::TurnOn);
            }
        }
        FolderSyncPhase::On | FolderSyncPhase::Paused if screen.ended.is_some() => {
            alert(ui, screen.ended.as_deref().unwrap_or_default());
        }
        FolderSyncPhase::On | FolderSyncPhase::Paused => {
            let paused = screen.phase == FolderSyncPhase::Paused;
            if screen.removal_hold {
                alert(
                    ui,
                    "A device left your account. Your changes wait until the workspace changes its key.",
                );
            }
            if screen.removal_over_limit {
                alert(
                    ui,
                    "A device left, but this workspace has 512 devices, its limit. That device reads new changes until someone leaves.",
                );
            }
            group(ui, "Status", |ui| {
                ui.label(if paused {
                    "Sync is paused"
                } else {
                    "Sync is on"
                });
                if screen.pending > 0 || screen.waiting_for_space == 0 {
                    ui.label(pending_text(screen.pending));
                }
                if screen.waiting_for_space > 0 {
                    ui.label(waiting_text(screen.waiting_for_space));
                }
                if let Some(usage) = &screen.usage {
                    ui.label(usage);
                }
                if screen.over_quota {
                    ui.label("Workspace full. New changes wait until you delete files.");
                }
                match &screen.last_success {
                    Some(when) => ui.label(format!("Last synced {when}")),
                    None => description(ui, "Not synced yet"),
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
                for conflict in &screen.conflicts {
                    ui.label(format!("{} has two versions. Keep:", conflict.path));
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
            if !screen.stopped_notes.is_empty() {
                group(ui, "Notes waiting for you", |ui| {
                    for note in &screen.stopped_notes {
                        ui.label(if note.outside {
                            format!(
                                "A rename took {} outside its shared folder, so its file stays where it is.",
                                note.path
                            )
                        } else {
                            format!(
                                "A collaborator's edit stopped {} from merging on this computer.",
                                note.path
                            )
                        });
                        if ui.button("Keep the last good version").clicked() {
                            actions.push(FolderSyncAction::KeepLastGood(note.rid));
                        }
                    }
                });
            }
            if !screen.text_reviews.is_empty() {
                group(ui, "Edits left out of notes", |ui| {
                    for line in &screen.text_reviews {
                        ui.label(line);
                    }
                });
            }
            if !screen.held.is_empty() {
                group(ui, "Versions to check", |ui| {
                    for held in &screen.held {
                        ui.label(format!("{} differs from the synced version:", held.path));
                        ui.horizontal_wrapped(|ui| {
                            for (label, choice) in [
                                ("Keep the synced version", HeldChoice::KeepLive),
                                ("Keep this computer's version", HeldChoice::KeepMine),
                                ("Keep both", HeldChoice::KeepBoth),
                            ] {
                                if ui.button(label).clicked() {
                                    actions.push(FolderSyncAction::KeepHeld {
                                        rid: held.rid,
                                        choice,
                                    });
                                }
                            }
                        });
                    }
                });
            }
            if !screen.held_roots.is_empty() {
                group(ui, "Folders waiting for you", |ui| {
                    for root in &screen.held_roots {
                        let folder = if root.folder.is_empty() {
                            "The workspace folder"
                        } else {
                            root.folder.as_str()
                        };
                        ui.label(format!(
                            "{folder} has a .noura-domain file. Syncing its files shows them to {}:",
                            people_text(root.people)
                        ));
                        for file in &root.files {
                            ui.label(file);
                        }
                        if ui.button(format!("Sync the files in {folder}")).clicked() {
                            actions.push(FolderSyncAction::ConfirmFolder(root.folder.clone()));
                        }
                        if let Some(domain) = root.restore
                            && ui
                                .button(format!("Restore the .noura-domain file in {folder}"))
                                .clicked()
                        {
                            actions.push(FolderSyncAction::RestoreMarker(domain));
                        }
                    }
                });
            }
            if !screen.archived.is_empty() {
                group(ui, "Versions from an earlier sync", |ui| {
                    for archived in &screen.archived {
                        ui.label(&archived.path);
                        ui.horizontal(|ui| {
                            if ui.button("Save as a copy").clicked() {
                                actions.push(FolderSyncAction::RestoreArchived(archived.index));
                            }
                            if ui.button("Discard").clicked() {
                                actions.push(FolderSyncAction::DiscardArchived(archived.index));
                            }
                        });
                    }
                });
            }
            if !screen.deleted_away.is_empty() {
                group(
                    ui,
                    "Deleted on another device while this one was away",
                    |ui| {
                        for path in &screen.deleted_away {
                            ui.label(path);
                            ui.horizontal(|ui| {
                                if ui.button("Delete").clicked() {
                                    actions
                                        .push(FolderSyncAction::ConfirmDeletedAway(path.clone()));
                                }
                                if ui.button("Keep").clicked() {
                                    actions.push(FolderSyncAction::KeepDeletedAway(path.clone()));
                                }
                            });
                        }
                    },
                );
            }
            if !screen.unreadable.is_empty() {
                group(ui, "Versions that couldn't be read", |ui| {
                    for line in &screen.unreadable {
                        ui.label(line);
                    }
                });
            }
            if !screen.skipped.is_empty() {
                group(ui, "Files sync leaves out", |ui| {
                    for line in &screen.skipped {
                        ui.label(line);
                    }
                });
            }
            if screen.can_set_up_again {
                set_up_again(ui, screen.busy, &mut actions);
            }
        }
        // The restore notice above offers the way back.
        FolderSyncPhase::AccountLost => {}
        FolderSyncPhase::NotConnected => {
            ui.label(
                "This folder copies a synced folder. Noura checks each file before it uploads anything.",
            );
            if primary_button(ui, "Connect this folder", !screen.busy).clicked() {
                actions.push(FolderSyncAction::Connect);
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
        assert_eq!(pending_text(0), "All changes uploaded");
        assert_eq!(pending_text(1), "1 change to upload");
        assert_eq!(pending_text(3), "3 changes to upload");
        assert_eq!(waiting_text(1), "1 change waits for space");
    }
}
