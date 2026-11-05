//! The share dialog (sync-v2.md §24): the expiry of a new link, Create
//! link, the note's links from this computer with Copy, Update and Revoke,
//! and every active link of the workspace with Revoke. The dialog only
//! draws what `app-core` reports and sends back what you do.

use app_core::App;
use app_core::shares::{DEFAULT_SHARE_DAYS, ShareAction};
use egui::{Button, Id, Modal, RichText, ScrollArea, Slider, Ui};

use crate::common::{muted, primary_button};
use crate::theme::{Palette, TITLE_SIZE};

/// Draws the share dialog while it's open, and copies a new link once.
pub(crate) fn dialog(ui: &mut Ui, app: &mut App) {
    let screen = app.share_screen();
    if !screen.open {
        return;
    }
    if let Some(link) = &screen.copy {
        ui.ctx().copy_text(link.0.to_string());
        app.share_action(ShareAction::Copied);
    }
    let palette = Palette::of(ui.ctx());
    let mut actions = Vec::new();
    let modal = Modal::new(Id::unique("share-link")).show(ui.ctx(), |ui| {
        ui.set_width(420.0);
        ui.label(
            RichText::new(format!("Share \u{201c}{}\u{201d}", screen.note))
                .size(TITLE_SIZE)
                .strong(),
        );
        if let Some(why) = &screen.unavailable {
            muted(ui, why);
            ui.add_space(8.0);
            if primary_button(ui, "Done", true).clicked() {
                actions.push(ShareAction::Close);
            }
            return;
        }
        muted(
            ui,
            "Anyone with the link can read a copy of this note as it is now, without an account. Images stay out.",
        );
        ui.add_space(8.0);
        let mut days = screen.days.max(1);
        let expiry = format!(
            "Expires in {days} {}",
            if days == 1 { "day" } else { "days" }
        );
        let slider = ui.add(
            Slider::new(&mut days, 1..=DEFAULT_SHARE_DAYS)
                .show_value(false)
                .text(expiry),
        );
        if slider.changed() {
            actions.push(ShareAction::SetDays(days));
        }
        if primary_button(ui, "Create and copy link", !screen.busy).clicked() {
            actions.push(ShareAction::Create);
        }
        if let Some(notice) = &screen.notice {
            ui.label(notice);
        }
        if let Some(problem) = &screen.problem {
            ui.label(RichText::new(problem).color(palette.destructive));
        }
        if !screen.links.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("This note's links").strong());
            for link in &screen.links {
                ui.horizontal(|ui| {
                    ui.label(format!("Expires {}, version {}", link.expires, link.revision));
                    if ui.add_enabled(!screen.busy, Button::new("Copy")).clicked() {
                        actions.push(ShareAction::Copy(link.token_hash));
                    }
                    if ui.add_enabled(!screen.busy, Button::new("Update")).clicked() {
                        actions.push(ShareAction::Update(link.token_hash));
                    }
                    if ui.add_enabled(!screen.busy, Button::new("Revoke")).clicked() {
                        actions.push(ShareAction::Revoke(link.token_hash));
                    }
                });
            }
        }
        if !screen.workspace_links.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("All links in this workspace").strong());
            muted(
                ui,
                "The server doesn't know which note each link shares, so links from other devices show by their ID.",
            );
            ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                for link in &screen.workspace_links {
                    ui.horizontal(|ui| {
                        if link.creator_removed {
                            ui.label(RichText::new(&link.label).color(palette.destructive))
                                .on_hover_text(
                                    "The device that made this link left the workspace. Revoke it unless you still want it.",
                                );
                        } else {
                            ui.label(&link.label);
                        }
                        if ui.add_enabled(!screen.busy, Button::new("Revoke")).clicked() {
                            actions.push(ShareAction::Revoke(link.token_hash));
                        }
                    });
                }
            });
        }
        ui.add_space(8.0);
        if ui.button("Done").clicked() {
            actions.push(ShareAction::Close);
        }
    });
    if modal.should_close() && !actions.contains(&ShareAction::Close) {
        actions.push(ShareAction::Close);
    }
    for action in actions {
        app.share_action(action);
    }
}
