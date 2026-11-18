//! The chooser: shown when no workspace is open, and when the last
//! workspace can't open. It is the legacy onboarding: an icon tile, a
//! title and a line, Open folder…, the recent workspaces and New
//! workspace…, centered in the window.

use app_core::RecentWorkspace;
use egui::{CentralPanel, RichText, Sense, Ui, WidgetInfo, accesskit::Role, vec2};

use crate::{
    NouraUi,
    common::{self, Btn, Size, Variant},
    icons,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, TITLE_SIZE, Weight, font, radius},
};

const WIDTH: f32 = 512.0 - 96.0;

pub(crate) fn show(state: &mut NouraUi, ui: &mut Ui, problem: Option<&str>) {
    CentralPanel::default().show(ui, |ui| {
        let width = ui.available_width().min(WIDTH);
        let side = (ui.available_width() - width) / 2.0;
        ui.add_space((ui.available_height() * 0.15).min(120.0));
        ui.horizontal(|ui| {
            ui.add_space(side);
            ui.vertical(|ui| {
                ui.set_width(width);
                contents(state, ui, problem);
            });
        });
    });
}

fn contents(state: &mut NouraUi, ui: &mut Ui, problem: Option<&str>) {
    let busy = state.app.is_busy();
    let palette = Palette::of(ui.ctx());
    let (title, description, icon) = match problem {
        Some(problem) => ("Can\u{2019}t open the workspace", problem, icons::WARNING),
        None => (
            "Open a folder",
            "Noura adds a .noura folder to it. Your files stay as they are.",
            icons::FOLDER_OPEN,
        ),
    };
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
        ui.painter().rect_filled(rect, radius(1.4), palette.muted);
        let glyph = egui::Rect::from_center_size(rect.center(), vec2(20.0, 20.0));
        icons::paint(ui.painter(), icon, glyph, palette.foreground, false);
        ui.add_space(8.0);
        ui.label(RichText::new(title).font(font(ui.ctx(), TITLE_SIZE, Weight::Medium)));
        ui.add(
            egui::Label::new(
                RichText::new(description)
                    .size(BODY_SIZE)
                    .color(palette.muted_foreground),
            )
            .wrap(),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let buttons = if problem.is_some() { 230.0 } else { 140.0 };
            ui.add_space(((ui.available_width() - buttons) / 2.0).max(0.0));
            let icon = if busy {
                icons::SPINNER
            } else {
                icons::FOLDER_OPEN
            };
            if Btn::new("Open folder\u{2026}")
                .icon(icon)
                .variant(Variant::Default)
                .enabled(!busy)
                .show(ui)
                .clicked()
            {
                state.pick_and_open();
            }
            if problem.is_some() && Btn::new("Try again").enabled(!busy).show(ui).clicked() {
                state.app.try_again();
            }
            if busy {
                ui.spinner().accessible_name("Opening workspace");
            }
        });
    });
    ui.add_space(16.0);
    common::notice(state, ui);

    let recents = state.app.recents().to_vec();
    if !recents.is_empty() {
        ui.separator();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Recent")
                    .size(SUPPORTING_SIZE)
                    .color(palette.muted_foreground),
            );
        });
        ui.add_space(4.0);
        for recent in &recents {
            if recent_row(ui, recent, !busy).clicked() {
                state.app.open_recent(&recent.workspace_id);
            }
        }
        ui.add_space(8.0);
    }

    ui.separator();
    ui.add_space(8.0);
    let mut create = false;
    ui.vertical_centered(|ui| {
        create = Btn::new("New workspace\u{2026}")
            .variant(Variant::Ghost)
            .size(Size::Sm)
            .enabled(!busy)
            .show(ui)
            .clicked();
    });
    if create {
        state.pick_and_create();
    }
}

/// One recent workspace: a ghost button with its name, and its folder
/// below in 12 px.
fn recent_row(ui: &mut Ui, recent: &RecentWorkspace, enabled: bool) -> egui::Response {
    let palette = Palette::of(ui.ctx());
    let width = ui.available_width();
    let name = egui::WidgetText::from(RichText::new(&recent.name).font(font(
        ui.ctx(),
        BODY_SIZE,
        Weight::Medium,
    )))
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        width - 16.0,
        egui::TextStyle::Body,
    );
    let path = egui::WidgetText::from(RichText::new(&recent.path).size(SUPPORTING_SIZE))
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            width - 16.0,
            egui::TextStyle::Small,
        );
    let height = 8.0 + name.size().y + path.size().y + 8.0;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), sense);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &recent.name));
    let described = recent.path.clone();
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_description(described));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if enabled && response.hovered() {
            painter.rect_filled(rect, crate::theme::pill(), palette.muted);
        }
        if response.has_focus() {
            common::focus_ring(painter, rect, crate::theme::pill(), &palette);
        }
        let at = rect.min + vec2(8.0, 8.0);
        let below = at + vec2(0.0, name.size().y);
        painter.galley(at, name, palette.foreground);
        painter.galley(below, path, palette.muted_foreground);
    }
    response
}
