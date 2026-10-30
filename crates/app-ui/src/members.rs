//! The Members section of Settings (sync-v2.md §8, §11): the workspace's
//! members with their roles and the buttons your role allows, the open
//! invitations with their role and status, Copy link and Revoke, the
//! Invite buttons, and Leave workspace. It draws what `app-core`'s members
//! screen holds and hands each choice to it.

use app_core::App;
use app_core::members::{Id, MembersAction, MembersScreen, Role, role_name};
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
    /// The change waiting for you to confirm.
    confirm: Option<Confirm>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Confirm {
    Remove(Id, String),
    Transfer(Id, String),
    Leave,
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
        };
        let modal = Modal::new(egui::Id::new("members-confirm")).show(ui.ctx(), |ui| {
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
