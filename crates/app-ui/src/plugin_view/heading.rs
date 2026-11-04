//! Page and section headings, and what an empty page or section shows.

use app_core::{HeadingLevel, PluginIcon, ViewKind, ViewNode};
use egui::{Align, Layout, Rect, RichText, Sense, Ui, UiBuilder, accesskit::Role, vec2};

use super::{
    Kit,
    menu::{Look, styled_button},
};
use crate::{
    icons,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, radius},
};

/// The size of a page heading's title.
const PAGE_TITLE_SIZE: f32 = 20.0;

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

/// A page's or a section's title, with its buttons at its end.
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
    let builder = UiBuilder::new()
        .accessibility_role(Role::Heading)
        .accessibility_label(text.as_str());
    ui.scope_builder(builder, |ui| {
        ui.horizontal(|ui| {
            if let Some(symbol) = symbol {
                let side = if page { 20.0 } else { 16.0 };
                let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
                icons::paint(
                    ui.painter(),
                    icon(*symbol),
                    rect,
                    palette.muted_foreground,
                    false,
                );
            }
            let size = if page { PAGE_TITLE_SIZE } else { BODY_SIZE };
            let title = RichText::new(text)
                .size(size)
                .strong()
                .color(palette.foreground);
            match detail {
                // A page heading's detail goes under its title.
                Some(detail) if page => {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
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
                // Right to left, so the first button goes last.
                for (index, child) in node.children.iter().enumerate().rev() {
                    if let ViewKind::Button { label } = &child.kind {
                        let look = match (page, index) {
                            (true, 0) => Look::Primary,
                            (true, _) => Look::Plain,
                            (false, _) => Look::Quiet,
                        };
                        ui.push_id(&child.id, |ui| styled_button(ui, child, label, look, kit));
                    }
                }
            });
        });
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
        ui.add_space(32.0);
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            if let Some(symbol) = symbol {
                let (rect, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
                ui.painter().rect_filled(rect, radius(2.0), palette.muted);
                let glyph = Rect::from_center_size(rect.center(), vec2(22.0, 22.0));
                icons::paint(
                    ui.painter(),
                    icon(*symbol),
                    glyph,
                    palette.foreground,
                    false,
                );
                ui.add_space(4.0);
            }
            ui.label(RichText::new(title).size(BODY_SIZE + 2.0).strong());
            if let Some(detail) = detail {
                ui.label(RichText::new(detail).color(palette.muted_foreground));
            }
            if !node.children.is_empty() {
                ui.add_space(4.0);
                for child in &node.children {
                    if let ViewKind::Button { label } = &child.kind {
                        ui.push_id(&child.id, |ui| {
                            styled_button(ui, child, label, Look::Primary, kit);
                        });
                    }
                }
            }
        });
        ui.add_space(32.0);
    });
}
