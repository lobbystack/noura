//! The Sync account section of Settings: the server address, signing in
//! through your browser, approving a new device, your devices with their
//! full fingerprints, removing one, the recovery kit and signing out
//! (docs/workspace-format/sync-v2.md §7, §10, §12). It draws what
//! `app-core`'s account screen holds and hands each choice to it.

use std::path::{Path, PathBuf};

use app_core::App;
use app_core::account::{AccountAction, AccountPhase, AccountScreen, RecoveryView, SignedInView};
use app_core::sync::recovery::KIT_WARNING;
use egui::{Button, RichText, TextEdit, Ui, UiBuilder, accesskit::Role};

use crate::FolderPicker;
use crate::common::muted;
use crate::theme::{Palette, TITLE_SIZE};

/// What the section remembers between frames.
#[derive(Default)]
pub(crate) struct AccountView {
    /// The address being typed, once you start typing.
    server: Option<String>,
    /// The recovery kit file and secret being typed.
    kit_path: String,
    kit_secret: String,
}

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

/// A code both screens show, large enough to compare at a glance.
fn code(ui: &mut Ui, code: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(code)
                .size(TITLE_SIZE * 1.4)
                .monospace()
                .strong(),
        )
        .selectable(true),
    );
}

/// A fingerprint in full, which you can select and copy.
fn fingerprint_line(ui: &mut Ui, fingerprint: &str) {
    ui.add(egui::Label::new(RichText::new(fingerprint).monospace()).selectable(true));
}

/// A new file name for the kit in `folder`.
fn kit_file(folder: &Path) -> PathBuf {
    let first = folder.join("Noura recovery kit.noura-kit");
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| folder.join(format!("Noura recovery kit {n}.noura-kit")))
        .find(|path| !path.exists())
        .expect("a free name")
}

/// Draws the Sync account section.
pub(crate) fn section(
    ui: &mut Ui,
    app: &mut App,
    view: &mut AccountView,
    picker: &mut dyn FolderPicker,
) {
    let screen = app.account();
    let mut actions = Vec::new();
    if let Some(problem) = &screen.problem {
        alert(ui, problem);
    }
    if screen.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            muted(ui, "Working on it.");
        });
    }
    match &screen.phase {
        AccountPhase::SignedOut => signed_out(ui, &screen, view, &mut actions),
        AccountPhase::SigningIn { user_code } => {
            group(ui, "Signing in", |ui| {
                ui.label("Your browser shows the sync server's sign-in page. Sign in there, check that it shows this code, and approve it:");
                code(ui, user_code);
                if ui.button("Cancel").clicked() {
                    actions.push(AccountAction::CancelSignIn);
                }
            });
        }
        AccountPhase::WaitingForApproval {
            fingerprint,
            code: shown,
            confirmed,
        } => {
            group(ui, "Approve this computer", |ui| {
                ui.label("On a device you already use, open Settings, then Sync account, and approve this computer's request.");
                ui.label("This computer's fingerprint:");
                fingerprint_line(ui, fingerprint);
                match shown {
                    None => {
                        muted(ui, "Waiting for your other device.");
                    }
                    Some(shown) => {
                        ui.label("Compare this code with the one on your other device:");
                        code(ui, shown);
                        if *confirmed {
                            muted(ui, "Waiting for your other device to finish.");
                        } else if ui.button("Codes match").clicked() {
                            actions.push(AccountAction::ConfirmCode);
                        }
                    }
                }
                if ui.button("Cancel").clicked() {
                    actions.push(AccountAction::CancelSignIn);
                }
            });
        }
        AccountPhase::Recovering(review) => recovering(ui, review, &mut actions),
        AccountPhase::SignedIn(signed_in) => {
            signed_in_section(ui, &screen, signed_in, &mut actions, picker);
        }
        AccountPhase::Removed { by } => {
            group(ui, "This computer was removed", |ui| {
                ui.label("A device of your account removed this computer, so it no longer syncs. Your files stay on this computer.");
                if !by.is_empty() {
                    ui.label("Removed by the device with this fingerprint:");
                    fingerprint_line(ui, by);
                }
            });
        }
    }
    for action in actions {
        app.account_action(action);
    }
}

fn signed_out(
    ui: &mut Ui,
    screen: &AccountScreen,
    view: &mut AccountView,
    actions: &mut Vec<AccountAction>,
) {
    group(ui, "Server address", |ui| {
        let server = view.server.get_or_insert_with(|| screen.server.clone());
        let response = ui
            .add(
                TextEdit::singleline(server)
                    .hint_text("https://sync.noura.app")
                    .desired_width(320.0),
            )
            .accessible_name("Server address");
        if response.changed() {
            actions.push(AccountAction::SetServer(server.clone()));
        }
        muted(
            ui,
            "Noura syncs through this server. Use https:// unless the server runs on this computer.",
        );
        if let Some(problem) = &screen.server_problem {
            alert(ui, problem);
        }
    });
    group(ui, "Sign in", |ui| {
        muted(
            ui,
            "Your browser opens the server's sign-in page. Noura never sees your passkey.",
        );
        let ready = screen.server_problem.is_none() && !screen.server.trim().is_empty();
        if ui
            .add_enabled(
                ready && !screen.busy,
                Button::new("Sign in with your browser"),
            )
            .clicked()
        {
            actions.push(AccountAction::SignIn);
        }
    });
    group(ui, "Recover with a kit", |ui| {
        muted(
            ui,
            "Lost every device? Open the recovery kit you exported and type its secret.",
        );
        ui.add(TextEdit::singleline(&mut view.kit_path).desired_width(320.0))
            .accessible_name("Kit file");
        ui.add(
            TextEdit::singleline(&mut view.kit_secret)
                .password(true)
                .desired_width(320.0),
        )
        .accessible_name("Kit secret");
        let ready = !view.kit_path.trim().is_empty() && !view.kit_secret.trim().is_empty();
        if ui
            .add_enabled(ready && !screen.busy, Button::new("Recover"))
            .clicked()
        {
            actions.push(AccountAction::Recover {
                path: PathBuf::from(view.kit_path.trim()),
                secret: std::mem::take(&mut view.kit_secret),
            });
        }
    });
}

fn recovering(ui: &mut Ui, review: &RecoveryView, actions: &mut Vec<AccountAction>) {
    group(ui, "Review your account", |ui| {
        ui.label("These changes happened after you exported the kit. Recover only if you recognize them.");
        if review.changes.is_empty() {
            muted(ui, "Nothing changed since the kit.");
        }
        for change in &review.changes {
            ui.label(change);
        }
    });
    group(ui, "Your other devices", |ui| {
        ui.label("Recovering removes these devices. Keep the ones you still hold.");
        for (device, fingerprint) in &review.devices {
            let mut keep = review.keep.contains(device);
            if ui
                .checkbox(&mut keep, format!("Keep {fingerprint}"))
                .changed()
            {
                actions.push(AccountAction::ToggleKeep(*device));
            }
        }
        if ui.button("Recover this account").clicked() {
            actions.push(AccountAction::ConfirmRecovery);
        }
    });
}

fn signed_in_section(
    ui: &mut Ui,
    screen: &AccountScreen,
    view: &SignedInView,
    actions: &mut Vec<AccountAction>,
    picker: &mut dyn FolderPicker,
) {
    group(ui, "Server", |ui| {
        ui.add(egui::Label::new(&screen.server).selectable(true));
    });
    group(ui, "Account", |ui| {
        ui.label("Your account's fingerprint:");
        fingerprint_line(ui, &view.account_fingerprint);
    });
    for warning in &view.alerts {
        alert(ui, warning);
    }
    if !view.requests.is_empty() || view.approval.is_some() {
        group(ui, "New devices", |ui| match &view.approval {
            Some(approval) => {
                ui.label("Approving the device with this fingerprint:");
                fingerprint_line(ui, &approval.fingerprint);
                match &approval.code {
                    None => {
                        muted(ui, "Waiting for the new device to show its code.");
                    }
                    Some(shown) => {
                        ui.label("Compare this code with the one on the new device:");
                        code(ui, shown);
                        if ui.button("Codes match").clicked() {
                            actions.push(AccountAction::ConfirmApproval);
                        }
                    }
                }
                if ui.button("Cancel").clicked() {
                    actions.push(AccountAction::CancelApproval);
                }
            }
            None => {
                ui.label("These devices wait for your approval. Approve only a device you're signing in yourself.");
                for (index, request) in view.requests.iter().enumerate() {
                    ui.horizontal(|ui| {
                        fingerprint_line(ui, &request.fingerprint);
                        if ui
                            .add(Button::new("Approve"))
                            .on_hover_text("Shows a code to compare on both devices")
                            .clicked()
                        {
                            actions.push(AccountAction::Approve(index));
                        }
                    });
                }
            }
        });
    }
    group(ui, "Devices", |ui| {
        for device in &view.devices {
            ui.horizontal_wrapped(|ui| {
                let title = match (device.this_device, device.recovery, device.removed) {
                    (true, _, _) => "This computer",
                    (_, true, true) => "Revoked recovery kit",
                    (_, true, false) => "Recovery kit",
                    (_, false, true) => "Removed device",
                    _ => "Device",
                };
                ui.label(RichText::new(title).strong());
                fingerprint_line(ui, &device.fingerprint);
                muted(ui, &device.how);
                if !device.this_device && !device.removed {
                    let label = if device.recovery { "Revoke" } else { "Remove" };
                    if ui.button(label).clicked() {
                        actions.push(AccountAction::Remove(device.device));
                    }
                }
            });
        }
    });
    if let Some(removal) = &view.removal {
        group(ui, "Remove a device", |ui| {
            ui.label("This removes the device with this fingerprint from your account. It can't read anything written after.");
            fingerprint_line(ui, &removal.fingerprint);
            if !removal.with_it.is_empty() {
                ui.label(
                    "These devices go with it, because it approved them. Keep any you recognize:",
                );
                for (device, fingerprint, keepable) in &removal.with_it {
                    let mut keep = removal.keep.contains(device);
                    if ui
                        .add_enabled(
                            *keepable,
                            egui::Checkbox::new(&mut keep, format!("Keep {fingerprint}")),
                        )
                        .changed()
                    {
                        actions.push(AccountAction::ToggleKeep(*device));
                    }
                }
            }
            ui.horizontal(|ui| {
                if ui.button("Remove").clicked() {
                    actions.push(AccountAction::ConfirmRemove);
                }
                if ui.button("Cancel").clicked() {
                    actions.push(AccountAction::CancelRemove);
                }
            });
        });
    }
    group(ui, "Recovery kit", |ui| match &view.kit {
        Some(kit) => {
            ui.label(format!("Noura saved your kit to {}.", kit.path.display()));
            ui.label("Write this secret down now. Noura won't show it again:");
            code(ui, &kit.secret);
            if ui.button("I wrote it down").clicked() {
                actions.push(AccountAction::DismissKit);
            }
        }
        None => {
            ui.label(KIT_WARNING);
            if view.offer_kit {
                muted(
                    ui,
                    "Your devices changed. Export a new kit so it covers them.",
                );
            }
            if ui.button("Export recovery kit").clicked()
                && let Some(folder) = picker.pick_folder("Choose where to save the recovery kit")
            {
                actions.push(AccountAction::ExportKit(kit_file(&folder)));
            }
        }
    });
    group(ui, "Sign out", |ui| match &view.sign_out {
        Some(confirm) => {
            if confirm.last_device {
                alert(
                    ui,
                    "This is your account's last device. After you sign out, you need your recovery kit, or a reset of your devices, to read your synced data again.",
                );
            }
            ui.label("Signing out stops sync on this computer and leaves your files as they are.");
            ui.horizontal(|ui| {
                if ui.button("Sign out").clicked() {
                    actions.push(AccountAction::ConfirmSignOut);
                }
                if ui.button("Stay signed in").clicked() {
                    actions.push(AccountAction::CancelSignOut);
                }
            });
        }
        None => {
            if ui.button("Sign out").clicked() {
                actions.push(AccountAction::SignOut);
            }
        }
    });
}
