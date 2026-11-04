//! Page and section headings, and what an empty page or section shows.

use app_core::{HeadingLevel, PluginIcon, ViewKind, ViewNode};
use egui::{Align, Layout, Rect, RichText, Sense, Ui, UiBuilder, accesskit::Role, vec2};

use super::{
    Kit,
    menu::{Look, styled_button},
};
use crate::{
    icons,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, TITLE_SIZE, Weight, font, radius},
};

fn icon(icon: PluginIcon) -> icons::Icon {
    match icon {
        PluginIcon::Calendar => icons::CALENDAR,
        PluginIcon::Done => icons::DONE,
        PluginIcon::Folder => icons::FOLDER,
        PluginIcon::Note => icons::NOTE,
        PluginIcon::Sparkle => icons::AI,
        PluginIcon::Tasks => icons::TASKS,
    }
}

/// A page's or a section's title, with its buttons at its end. A page
/// heading is the legacy page header: its 14 px semibold title with the
/// detail under it, over a line. A section heading has its icon, its 14 px
/// medium title and its detail beside it.
pub(super) fn heading(ui: &mut Ui, node: &ViewNode, kit: &mut Kit) {
    let ViewKind::Heading {
        text,
        level,
        detail,
        icon: symbol,
    } = &node.kind
    else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let page = *level == HeadingLevel::Page;
    if page {
        kit.page_detail = detail.is_some();
    }
    let builder = UiBuilder::new()
        .accessibility_role(Role::Heading)
        .accessibility_label(text.as_str());
    ui.scope_builder(builder, |ui| {
        let height = if page { 32.0 } else { 20.0 };
        ui.horizontal(|ui| {
            ui.set_min_height(height);
            ui.spacing_mut().item_spacing.x = 8.0;
            if let Some(symbol) = symbol.filter(|_| !page) {
                let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                icons::paint(
                    ui.painter(),
                    icon(symbol),
                    rect,
                    palette.muted_foreground,
                    false,
                );
            }
            let weight = if page {
                Weight::Semibold
            } else {
                Weight::Medium
            };
            let title = RichText::new(text)
                .font(font(ui.ctx(), BODY_SIZE, weight))
                .color(palette.foreground);
            match detail {
                // A page heading's detail goes under its title.
                Some(detail) if page => {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(title);
                        ui.label(
                            RichText::new(detail)
                                .size(SUPPORTING_SIZE)
                                .color(palette.muted_foreground),
                        );
                    });
                }
                Some(detail) => {
                    ui.label(title);
                    ui.label(
                        RichText::new(detail)
                            .size(SUPPORTING_SIZE)
                            .color(palette.muted_foreground),
                    );
                }
                None => {
                    ui.label(title);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                // Right to left, so the first child goes last.
                for (index, child) in node.children.iter().enumerate().rev() {
                    match &child.kind {
                        ViewKind::Button { label } => {
                            let look = match (page, index) {
                                (true, 0) => Look::Primary,
                                (true, _) => Look::Ghost,
                                (false, _) => Look::Quiet,
                            };
                            ui.push_id(&child.id, |ui| {
                                styled_button(ui, child, label, look, kit);
                            });
                        }
                        ViewKind::Input { .. } => {
                            ui.push_id(&child.id, |ui| super::form::input(ui, child, kit));
                        }
                        _ => {}
                    }
                }
            });
        });
        if page {
            // The header's line runs the page's whole width.
            let rect = ui.min_rect();
            let y = rect.bottom() + 12.0;
            let width = ui.max_rect().x_range().expand(24.0);
            ui.painter()
                .hline(width, y, egui::Stroke::new(1.0, palette.border));
            ui.add_space(24.0);
        }
    });
}

/// What an empty page or section shows: its icon in a tile, its title,
/// a line that says what to do and its buttons, centered.
pub(super) fn empty(ui: &mut Ui, node: &ViewNode, kit: &mut Kit) {
    let ViewKind::Empty {
        title,
        detail,
        icon: symbol,
    } = &node.kind
    else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    // The title names it, so the group adds no name of its own.
    let builder = UiBuilder::new().accessibility_role(Role::Group);
    ui.scope_builder(builder, |ui| {
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            if let Some(symbol) = symbol {
                let (rect, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                ui.painter().rect_filled(rect, radius(1.4), palette.muted);
                let glyph = Rect::from_center_size(rect.center(), vec2(20.0, 20.0));
                icons::paint(
                    ui.painter(),
                    icon(*symbol),
                    glyph,
                    palette.foreground,
                    false,
                );
                ui.add_space(8.0);
            }
            ui.label(RichText::new(title).font(font(ui.ctx(), TITLE_SIZE, Weight::Medium)));
            if let Some(detail) = detail {
                ui.scope(|ui| {
                    ui.set_max_width(384.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(detail)
                                .size(BODY_SIZE)
                                .color(palette.muted_foreground),
                        )
                        .wrap(),
                    );
                });
            }
            if !node.children.is_empty() {
                ui.add_space(8.0);
                for child in &node.children {
                    if let ViewKind::Button { label } = &child.kind {
                        ui.push_id(&child.id, |ui| {
                            styled_button(ui, child, label, Look::Primary, kit);
                        });
                    }
                }
            }
        });
        ui.add_space(48.0);
    });
}
