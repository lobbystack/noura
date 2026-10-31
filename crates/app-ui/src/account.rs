//! The Sync account section of Settings: the server address, signing in
//! through your browser, approving a new device, your devices with their
//! full fingerprints, removing one, the recovery kit and signing out
//! (docs/workspace-format/sync-v2.md §7, §10, §12). It draws what
//! `app-core`'s account screen holds and hands each choice to it.

use std::path::{Path, PathBuf};

use app_core::App;
use app_core::account::{
    AccountAction, AccountPhase, AccountScreen, InvitationStage, InvitationView, RecoveryView,
    SignedInView,
};
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
    /// The invitation link being pasted.
    invitation_link: String,
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
    let mut download = None;
    if let Some(invitation) = &screen.invitation {
        download = invitation_group(ui, &screen, invitation, &mut actions, picker);
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
                muted(ui, "Lost every device and your recovery kit?");
                if ui.button("Reset devices").clicked() {
                    actions.push(AccountAction::StartReset);
                }
            });
        }
        AccountPhase::ResetCode => {
            group(ui, "Reset your devices", |ui| {
                ui.label("We emailed you a code of 8 characters. It works once, for 10 minutes.");
                let id = ui.make_persistent_id("reset-code");
                let mut typed: String = ui.data_mut(|data| data.get_temp(id).unwrap_or_default());
                ui.add(TextEdit::singleline(&mut typed).hint_text("Reset code"));
                ui.data_mut(|data| data.insert_temp(id, typed.clone()));
                ui.horizontal(|ui| {
                    let ready = !typed.trim().is_empty();
                    if ui
                        .add_enabled(ready, Button::new("Start over with this code"))
                        .clicked()
                    {
                        actions.push(AccountAction::ConfirmReset(typed.clone()));
                    }
                    if ui.button("Cancel").clicked() {
                        actions.push(AccountAction::CancelSignIn);
                    }
                });
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
                muted(
                    ui,
                    "To sync here again, sign in as a new device and approve it from a device you hold.",
                );
                if ui
                    .add_enabled(!screen.busy, Button::new("Sign in as a new device"))
                    .clicked()
                {
                    actions.push(AccountAction::SignIn);
                }
            });
        }
    }
    if screen.invitation.is_none()
        && matches!(
            screen.phase,
            AccountPhase::SignedOut | AccountPhase::SignedIn(_)
        )
    {
        paste_invitation(ui, &screen, view, &mut actions);
    }
    for action in actions {
        app.account_action(action);
    }
    if let (Some(folder), Some(invitation)) = (download, &screen.invitation) {
        app.download_invited(&invitation.workspace, folder);
    }
}

fn paste_invitation(
    ui: &mut Ui,
    screen: &AccountScreen,
    view: &mut AccountView,
    actions: &mut Vec<AccountAction>,
) {
    group(ui, "Join a workspace", |ui| {
        ui.add(
            TextEdit::singleline(&mut view.invitation_link)
                .hint_text("https://sync.noura.app/invite/...")
                .desired_width(320.0),
        )
        .accessible_name("Invitation link");
        let filled = !view.invitation_link.trim().is_empty();
        if ui
            .add_enabled(!screen.busy && filled, Button::new("Open link"))
            .clicked()
        {
            actions.push(AccountAction::OpenInvitation(std::mem::take(
                &mut view.invitation_link,
            )));
        }
    });
}

/// "a viewer" or "an editor".
fn with_article(role: &str) -> String {
    let article = if role.starts_with(['a', 'e']) {
        "an"
    } else {
        "a"
    };
    format!("{article} {role}")
}

/// Draws the invitation you opened. Returns the folder you chose for the
/// workspace you joined.
fn invitation_group(
    ui: &mut Ui,
    screen: &AccountScreen,
    invitation: &InvitationView,
    actions: &mut Vec<AccountAction>,
    picker: &mut dyn FolderPicker,
) -> Option<PathBuf> {
    let mut chosen = None;
    let inviter = &invitation.inviter;
    group(ui, "Invitation", |ui| {
        ui.label(format!(
            "{inviter} invited you to {} as {}.",
            invitation.workspace,
            with_article(&invitation.role)
        ));
        match &invitation.stage {
            InvitationStage::Opened => {
                let signed_in = matches!(screen.phase, AccountPhase::SignedIn(_));
                let accept = if signed_in {
                    "Accept"
                } else {
                    "Sign in and accept"
                };
                if ui.add_enabled(!screen.busy, Button::new(accept)).clicked() {
                    actions.push(AccountAction::AcceptInvitation);
                }
                if ui.button("Not now").clicked() {
                    actions.push(AccountAction::CancelInvitation);
                }
            }
            InvitationStage::Waiting {
                fingerprint,
                code: shown,
                confirmed,
            } => {
                ui.label("This computer's fingerprint:");
                fingerprint_line(ui, fingerprint);
                match shown {
                    None => {
                        muted(
                            ui,
                            &format!("Waiting for {inviter} to approve this computer."),
                        );
                    }
                    Some(shown) => {
                        ui.label(format!(
                            "Compare this code with {inviter} on a call or another channel you trust:"
                        ));
                        code(ui, shown);
                        if *confirmed {
                            muted(ui, &format!("Waiting for {inviter}'s computer to finish."));
                        } else if ui.button("Codes match").clicked() {
                            actions.push(AccountAction::ConfirmInvitationCode);
                        }
                    }
                }
                if ui.button("Cancel").clicked() {
                    actions.push(AccountAction::CancelInvitation);
                }
            }
            InvitationStage::Joined { downloading } => {
                if *downloading {
                    muted(ui, "Noura is downloading the workspace.");
                } else {
                    ui.label("You joined. Choose an empty folder for its files.");
                    if ui
                        .add_enabled(!screen.busy, Button::new("Choose a folder"))
                        .clicked()
                    {
                        chosen = picker.pick_folder("Choose an empty folder for the workspace");
                    }
                }
            }
        }
    });
    chosen
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
    if screen.can_retry {
        group(ui, "Connect again", |ui| {
            muted(
                ui,
                "This computer holds a device of your account that couldn't connect.",
            );
            if ui
                .add_enabled(!screen.busy, Button::new("Try again"))
                .clicked()
            {
                actions.push(AccountAction::Retry);
            }
        });
    }
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
    if !review.members.is_empty() {
        group(ui, "Workspaces your kit doesn't list", |ui| {
            ui.label("Noura checked these workspaces against your account. Recover only if you know every member:");
            for line in &review.members {
                ui.label(line);
            }
        });
    }
    if !review.unverified.is_empty() {
        group(ui, "Workspaces Noura couldn't check", |ui| {
            for line in &review.unverified {
                alert(ui, line);
            }
        });
    }
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

/// What a reset asks of you: your new fingerprint and each workspace that
/// has to accept it, and the resets you can accept here (§7).
fn reset_groups(ui: &mut Ui, view: &SignedInView, actions: &mut Vec<AccountAction>) {
    if let Some(shown) = &view.reset_fingerprint {
        group(ui, "You reset your devices", |ui| {
            ui.label("This account's new fingerprint:");
            fingerprint_line(ui, shown);
            ui.label("Approve your other devices again, and export a new recovery kit.");
        });
    }
    if !view.reset_waits.is_empty() {
        group(ui, "Workspaces that have to accept the reset", |ui| {
            ui.label("An owner or admin of each workspace accepts it. Compare the code with them.");
            for (index, wait) in view.reset_waits.iter().enumerate() {
                ui.label(format!("Workspace {}", wait.workspace));
                match &wait.code {
                    None => {
                        muted(ui, "Waiting for an owner or admin.");
                    }
                    Some(shown) => {
                        code(ui, shown);
                        if wait.confirmed {
                            muted(ui, "Waiting for them to finish.");
                        } else if ui.button("Codes match").clicked() {
                            actions.push(AccountAction::ConfirmResetWait(index));
                        }
                    }
                }
            }
        });
    }
    if let Some(approval) = &view.reset_approval {
        group(ui, "Accept this reset", |ui| {
            ui.label("The person's new device:");
            fingerprint_line(ui, &approval.fingerprint);
            match &approval.code {
                None => {
                    muted(ui, "Waiting for their device.");
                }
                Some(shown) => {
                    ui.label("Compare this code with theirs, over a channel you both trust:");
                    code(ui, shown);
                    if ui.button("Codes match, accept").clicked() {
                        actions.push(AccountAction::ConfirmAcceptReset);
                    }
                }
            }
            if ui.button("Cancel").clicked() {
                actions.push(AccountAction::CancelAcceptReset);
            }
        });
    } else if !view.reset_requests.is_empty() {
        group(ui, "Resets to accept", |ui| {
            for (index, request) in view.reset_requests.iter().enumerate() {
                fingerprint_line(ui, &request.fingerprint);
                if ui.button("Accept this reset").clicked() {
                    actions.push(AccountAction::AcceptReset(index));
                }
            }
        });
    }
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
    reset_groups(ui, view, actions);
    for warning in &view.alerts {
        alert(ui, &warning.text);
        if let Some(device) = warning.end_sessions
            && ui.button("Sign that device out").clicked()
        {
            actions.push(AccountAction::EndSessions(device));
        }
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
    if !view.removed_changes.is_empty() {
        group(ui, "Changes your removed devices made", |ui| {
            ui.label("Removing a device leaves its workspace changes in place. Check each one:");
            for change in &view.removed_changes {
                ui.label(change);
            }
        });
    }
    if !view.added_members.is_empty() {
        group(ui, "Members added since your kit", |ui| {
            ui.label("These accounts joined your workspaces after you exported the kit. Check that you know each one:");
            for line in &view.added_members {
                ui.label(line);
            }
        });
    }
    if let Some(replacement) = &view.kit_replacement {
        group(ui, "Replace your recovery kit", |ui| {
            ui.label("Your current kit recovered these devices. A new kit removes them, unless you keep the ones you hold:");
            for (device, fingerprint) in &replacement.devices {
                let mut keep = replacement.keep.contains(device);
                if ui
                    .checkbox(&mut keep, format!("Keep {fingerprint}"))
                    .changed()
                {
                    actions.push(AccountAction::ToggleKeep(*device));
                }
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!screen.busy, Button::new("Export the new kit"))
                    .clicked()
                {
                    actions.push(AccountAction::ConfirmExportKit);
                }
                if ui.button("Keep the current kit").clicked() {
                    actions.push(AccountAction::CancelExportKit);
                }
            });
        });
    }
    group(ui, "Recovery kit", |ui| match &view.kit {
        Some(kit) => {
            ui.label(format!("Noura saved your kit to {}.", kit.path.display()));
            ui.label("Write this secret down now. Noura won't show it again:");
            code(ui, kit.secret.as_str());
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
