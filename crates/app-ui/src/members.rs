//! The Members section of Settings (sync-v2.md §8, §11): the workspace's
//! members with their roles and the buttons your role allows, the open
//! invitations with their role and status, Copy link and Revoke, the
//! Invite buttons, and Leave workspace. It draws what `app-core`'s members
//! screen holds and hands each choice to it.

use app_core::App;
use app_core::members::{FolderEntry, Id, Level, MembersAction, MembersScreen, Role, role_name};
use egui::{Button, Modal, RichText, TextEdit, Ui, UiBuilder, accesskit};

use crate::common::{muted, primary_button};
use crate::theme::Palette;

/// What the section remembers between frames.
#[derive(Default)]
pub(crate) struct MembersView {
    /// Whether the section asked for the list since it opened.
    pub(crate) refreshed: bool,
    /// Your name, as invitees see it.
    your_name: String,
    /// The folder you're about to restrict.
    new_folder: String,
    /// The change waiting for you to confirm.
    confirm: Option<Confirm>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Confirm {
    Remove(Id, String),
    Transfer(Id, String),
    Leave,
    TakeOff(Id, Id, String),
    HandOff(Id, Id, String),
    DeleteFolder(Id, String),
}

fn group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    let builder = UiBuilder::new()
        .accessibility_role(accesskit::Role::Group)
        .accessibility_label(title);
    ui.scope_builder(builder, add);
}

fn alert(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(accesskit::Role::Alert)
        .accessibility_label(text);
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(text).color(palette.destructive));
    });
}

/// The button that gives a member `role`.
fn make_text(role: Role) -> String {
    match role {
        Role::Admin => "Make admin".into(),
        other => format!("Make {}", role_name(other).to_lowercase()),
    }
}

fn invite_text(role: Role) -> &'static str {
    match role {
        Role::Admin => "Invite an admin",
        Role::Editor => "Invite an editor",
        _ => "Invite a viewer",
    }
}

/// Draws the Members section.
pub(crate) fn section(ui: &mut Ui, app: &mut App, view: &mut MembersView) {
    let screen = app.members();
    if !view.refreshed {
        view.refreshed = true;
        app.members_action(MembersAction::Refresh);
    }
    if let Some(link) = &screen.copy {
        ui.ctx().copy_text(link.0.to_string());
        app.members_action(MembersAction::Copied);
    }
    let mut actions = Vec::new();
    draw(ui, &screen, view, &mut actions);
    for action in actions {
        app.members_action(action);
    }
}

fn draw(
    ui: &mut Ui,
    screen: &MembersScreen,
    view: &mut MembersView,
    actions: &mut Vec<MembersAction>,
) {
    if let Some(why) = &screen.unavailable {
        muted(ui, why);
        return;
    }
    if let Some(problem) = &screen.problem {
        alert(ui, problem);
    }
    if let Some(notice) = &screen.notice {
        ui.label(notice);
    }
    if screen.busy {
        ui.horizontal(|ui| {
            ui.spinner();
            muted(ui, "Working on it.");
        });
    }
    let idle = !screen.busy;
    if let Some(you) = screen.you {
        ui.label(format!("Your role: {}", role_name(you).to_lowercase()));
    }
    if let Some(confirm) = view.confirm.clone() {
        let (question, button, action) = match &confirm {
            Confirm::Remove(account, who) => (
                format!(
                    "Remove {who}? Their devices stop syncing this workspace, and Noura changes its key. They keep copies of files they already have."
                ),
                "Remove",
                MembersAction::Remove(*account),
            ),
            Confirm::Transfer(account, who) => (
                format!("Make {who} the owner? You become an admin."),
                "Make owner",
                MembersAction::TransferOwner(*account),
            ),
            Confirm::Leave => (
                "Leave this workspace? Your devices stop syncing it. Its files stay on this computer as they are."
                    .to_owned(),
                "Leave",
                MembersAction::Leave,
            ),
            Confirm::TakeOff(domain, account, who) => (
                format!(
                    "Take {who} off this folder? Noura changes its key, so they can't read what people write there next. Copies they already have stay with them."
                ),
                "Take off",
                MembersAction::TakeOff {
                    domain: *domain,
                    account: *account,
                },
            ),
            Confirm::HandOff(domain, account, who) => (
                format!(
                    "Hand this folder to {who}? They become a manager, and you lose access. Copies you already have stay on this computer."
                ),
                "Hand over",
                MembersAction::HandOff {
                    domain: *domain,
                    account: *account,
                },
            ),
            Confirm::DeleteFolder(domain, folder) => (
                format!(
                    "Delete the restriction on {folder}? Each device keeps the files and holds them until you confirm where they go."
                ),
                "Delete restriction",
                MembersAction::DeleteFolder(*domain),
            ),
        };
        let modal = Modal::new(egui::Id::unique("members-confirm")).show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            group(ui, "Confirm", |ui| {
                ui.label(question);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, button, idle).clicked() {
                        actions.push(action);
                        view.confirm = None;
                    }
                    if ui.button("Cancel").clicked() {
                        view.confirm = None;
                    }
                });
            });
        });
        if modal.should_close() {
            view.confirm = None;
        }
    }
    ui.add_space(8.0);
    ui.label(RichText::new("Members").strong());
    for member in &screen.members {
        group(ui, &member.label, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(&member.label);
                for role in &member.roles {
                    if ui
                        .add_enabled(idle, Button::new(make_text(*role)))
                        .clicked()
                    {
                        actions.push(MembersAction::SetRole {
                            account: member.account,
                            role: *role,
                        });
                    }
                }
                let who = member
                    .label
                    .split(',')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                if member.transferable && ui.add_enabled(idle, Button::new("Make owner")).clicked()
                {
                    view.confirm = Some(Confirm::Transfer(member.account, who.clone()));
                }
                if member.removable && ui.add_enabled(idle, Button::new("Remove")).clicked() {
                    view.confirm = Some(Confirm::Remove(member.account, who));
                }
            });
        });
    }
    if !screen.invite_roles.is_empty() {
        ui.add_space(8.0);
        ui.label(RichText::new("Invitations").strong());
        muted(
            ui,
            "An invitation link works for 7 days. Send it over a channel you trust. Only the computer that made it can approve the new member.",
        );
        ui.horizontal(|ui| {
            ui.label("Your name, as they see it");
            ui.add(
                TextEdit::singleline(&mut view.your_name)
                    .desired_width(200.0)
                    .hint_text("Your name"),
            );
        });
        ui.horizontal_wrapped(|ui| {
            for role in &screen.invite_roles {
                if ui
                    .add_enabled(idle, Button::new(invite_text(*role)))
                    .clicked()
                {
                    actions.push(MembersAction::Invite {
                        role: *role,
                        your_name: view.your_name.clone(),
                    });
                }
            }
        });
        if screen.invitations.is_empty() {
            muted(ui, "No open invitations.");
        }
        for invitation in &screen.invitations {
            group(ui, &invitation.label, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&invitation.label);
                    if invitation.sent_here
                        && ui.add_enabled(idle, Button::new("Copy link")).clicked()
                    {
                        actions.push(MembersAction::CopyLink(invitation.invitation));
                    }
                    if ui.add_enabled(idle, Button::new("Revoke")).clicked() {
                        actions.push(MembersAction::Revoke(invitation.invitation));
                    }
                });
                if !invitation.sent_here {
                    muted(
                        ui,
                        "Another computer sent this one. Only it can approve the new member.",
                    );
                }
                if let Some(warning) = &invitation.returning {
                    alert(ui, warning);
                }
            });
        }
    }
    approvals(ui, screen, actions, idle);
    restricted(ui, screen, view, actions, idle);
    if screen.may_leave {
        ui.add_space(8.0);
        if ui
            .add_enabled(idle, Button::new("Leave workspace"))
            .clicked()
        {
            view.confirm = Some(Confirm::Leave);
        }
    } else if screen.you == Some(Role::Owner) {
        ui.add_space(8.0);
        muted(
            ui,
            "To leave this workspace, make another member the owner first.",
        );
    }
}

/// The invited devices that wait for this computer, and the approval in
/// progress with the code both people compare (§10).
fn approvals(ui: &mut Ui, screen: &MembersScreen, actions: &mut Vec<MembersAction>, idle: bool) {
    if screen.requests.is_empty() && screen.approval.is_none() {
        return;
    }
    ui.add_space(8.0);
    ui.label(RichText::new("New members").strong());
    let Some(approval) = &screen.approval else {
        muted(
            ui,
            "Approve a device only after you check with the person you invited.",
        );
        for request in &screen.requests {
            group(ui, &request.label, |ui| {
                ui.label(&request.label);
                fingerprint_line(ui, &request.fingerprint);
                if let Some(warning) = &request.returning {
                    alert(ui, warning);
                }
                if ui.add_enabled(idle, Button::new("Approve")).clicked() {
                    actions.push(MembersAction::Approve(request.commit));
                }
            });
        }
        return;
    };
    group(ui, "Approving a new member", |ui| {
        ui.label("Approving the device with this fingerprint:");
        fingerprint_line(ui, &approval.fingerprint);
        if let Some(warning) = &approval.returning {
            alert(ui, warning);
        }
        match &approval.code {
            None => {
                muted(ui, "Waiting for their computer to show its code.");
            }
            Some(code) => {
                ui.label("Compare this code with theirs on a call or another channel you trust:");
                ui.add(
                    egui::Label::new(RichText::new(code).size(24.0).monospace().strong())
                        .selectable(true),
                );
                if ui.add_enabled(idle, Button::new("Codes match")).clicked() {
                    actions.push(MembersAction::ConfirmApproval);
                }
            }
        }
        if ui.add_enabled(idle, Button::new("Cancel")).clicked() {
            actions.push(MembersAction::CancelApproval);
        }
    });
}

fn fingerprint_line(ui: &mut Ui, fingerprint: &str) {
    ui.add(egui::Label::new(RichText::new(fingerprint).monospace()).selectable(true));
}

fn restricted(
    ui: &mut Ui,
    screen: &MembersScreen,
    view: &mut MembersView,
    actions: &mut Vec<MembersAction>,
    idle: bool,
) {
    if !screen.may_restrict
        && screen.folders.is_empty()
        && screen.moves.is_empty()
        && screen.held.is_empty()
        && screen.moved_in.is_none()
    {
        return;
    }
    ui.add_space(8.0);
    ui.label(RichText::new("Restricted folders").strong());
    if let Some(warning) = &screen.moved_in {
        alert(ui, warning);
        if ui.button("Dismiss").clicked() {
            actions.push(MembersAction::DismissMovedIn);
        }
    }
    for entry in &screen.moves {
        group(ui, &entry.label, |ui| {
            ui.label(&entry.label);
            if ui.add_enabled(idle, Button::new("Move it")).clicked() {
                actions.push(MembersAction::ConfirmMove(entry.rid));
            }
        });
    }
    for entry in &screen.held {
        group(ui, &entry.label, |ui| {
            ui.label(&entry.label);
            if ui.add_enabled(idle, Button::new("Confirm")).clicked() {
                actions.push(MembersAction::ConfirmHeld(entry.folder.clone()));
            }
        });
    }
    for folder in &screen.folders {
        folder_row(ui, folder, view, actions, idle);
    }
    if screen.may_restrict {
        ui.horizontal(|ui| {
            ui.label("Folder to restrict");
            ui.add(
                TextEdit::singleline(&mut view.new_folder)
                    .desired_width(200.0)
                    .hint_text("Path in the workspace"),
            );
            let filled = !view.new_folder.trim().is_empty();
            if ui
                .add_enabled(idle && filled, Button::new("Restrict"))
                .clicked()
            {
                actions.push(MembersAction::Restrict(std::mem::take(
                    &mut view.new_folder,
                )));
            }
        });
    }
}

fn folder_row(
    ui: &mut Ui,
    folder: &FolderEntry,
    view: &mut MembersView,
    actions: &mut Vec<MembersAction>,
    idle: bool,
) {
    group(ui, &folder.label, |ui| {
        ui.label(&folder.label);
        for grant in &folder.grants {
            let who = grant.label.split(',').next().unwrap_or_default().to_owned();
            ui.horizontal_wrapped(|ui| {
                ui.label(&grant.label);
                if grant.removable && ui.add_enabled(idle, Button::new("Take off")).clicked() {
                    view.confirm =
                        Some(Confirm::TakeOff(folder.domain, grant.account, who.clone()));
                }
                if grant.may_hand_off && ui.add_enabled(idle, Button::new("Hand over")).clicked() {
                    view.confirm =
                        Some(Confirm::HandOff(folder.domain, grant.account, who.clone()));
                }
            });
        }
        for candidate in &folder.candidates {
            ui.horizontal_wrapped(|ui| {
                ui.label(&candidate.label);
                for (text, level) in [("Let read", Level::Read), ("Let edit", Level::Write)] {
                    if ui.add_enabled(idle, Button::new(text)).clicked() {
                        actions.push(MembersAction::Grant {
                            domain: folder.domain,
                            account: candidate.account,
                            level,
                        });
                    }
                }
            });
            if let Some(warning) = &candidate.returning {
                alert(ui, warning);
            }
        }
        if folder.deletable
            && ui
                .add_enabled(idle, Button::new("Delete restriction"))
                .clicked()
        {
            view.confirm = Some(Confirm::DeleteFolder(folder.domain, folder.label.clone()));
        }
    });
}
