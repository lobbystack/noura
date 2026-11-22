//! Page and section headings, and what an empty page or section shows.

use app_core::{HeadingLevel, PluginIcon, ViewKind, ViewNode};
use egui::{Align, Layout, RichText, Sense, Ui, UiBuilder, accesskit::Role, text::LayoutJob, vec2};

use super::{
    Kit,
    menu::{Look, styled_button},
};
use crate::{
    icons,
    theme::{BODY_SIZE, PERIOD_TITLE_SIZE, Palette, SUPPORTING_SIZE, TITLE_SIZE, Weight, font},
};

fn icon(icon: PluginIcon) -> icons::Icon {
    match icon {
        PluginIcon::Calendar => icons::CALENDAR,
        PluginIcon::Done => icons::DONE,
        PluginIcon::Folder => icons::FOLDER,
        PluginIcon::Note => icons::NOTE,
        PluginIcon::Tasks => icons::TASKS,
    }
}

/// A page's or a section's title, with its buttons at its end. A page
/// heading is the page header: its 16 px semibold title, as a note's
/// header sets its name, with the detail under it, over a line. A period
/// heading is a page header too, its title large with the rest of it, the
/// detail, after it in regular weight, as a calendar names its month. A
/// section heading has its icon, its 14 px medium title and its detail
/// beside it.
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
    let period = *level == HeadingLevel::Period;
    let page = period || *level == HeadingLevel::Page;
    if page {
        kit.page_detail = detail.is_some();
        kit.page_period = period;
    }
    let label = match detail {
        Some(detail) if period => format!("{text} {detail}"),
        _ => text.clone(),
    };
    let builder = UiBuilder::new()
        .accessibility_role(Role::Heading)
        .accessibility_label(label);
    ui.scope_builder(builder, |ui| {
        // A page header's row fits its title and the detail under it, so
        // every page's buttons sit at one height.
        let height = if page { 36.0 } else { 20.0 };
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
            // A heading in a popover is its title, as large as an
            // editable title.
            let (size, weight) = if kit.in_popover {
                (TITLE_SIZE + 2.0, Weight::Semibold)
            } else if period {
                (PERIOD_TITLE_SIZE, Weight::Semibold)
            } else if page {
                (TITLE_SIZE, Weight::Semibold)
            } else {
                (BODY_SIZE, Weight::Medium)
            };
            let title = RichText::new(text)
                .font(font(ui.ctx(), size, weight))
                .color(palette.foreground);
            match detail {
                // A period heading's detail is the rest of its title.
                Some(detail) if period => {
                    let mut job = LayoutJob::default();
                    title.append_to(
                        &mut job,
                        ui.style(),
                        egui::FontSelection::Default,
                        Align::Center,
                    );
                    RichText::new(format!(" {detail}"))
                        .font(font(ui.ctx(), size, Weight::Regular))
                        .color(palette.foreground)
                        .append_to(
                            &mut job,
                            ui.style(),
                            egui::FontSelection::Default,
                            Align::Center,
                        );
                    // The buttons take their room first, from the end, and
                    // the title shortens to fit what's left in a narrow
                    // window.
                    let row = egui::Rect::from_min_size(
                        ui.cursor().min,
                        vec2(ui.available_width(), height),
                    );
                    let mut end = ui.new_child(
                        UiBuilder::new()
                            .max_rect(row)
                            .layout(Layout::right_to_left(Align::Center)),
                    );
                    controls(&mut end, node, page, kit);
                    let room = row.width() - end.min_rect().width() - 16.0;
                    ui.scope(|ui| {
                        ui.set_max_width(room.max(0.0));
                        ui.add(egui::Label::new(job).truncate());
                    });
                    return;
                }
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
                controls(ui, node, page, kit);
            });
        });
        if page {
            // The header's line runs the page's whole width.
            let rect = ui.min_rect();
            let y = rect.bottom() + 12.0;
            let width = ui.max_rect().x_range().expand(24.0);
            ui.painter()
                .hline(width, y, egui::Stroke::new(1.0, palette.border));
            // What follows starts at the line, as under the legacy header.
            let gap = y - ui.cursor().top();
            ui.add_space(gap.max(0.0));
        }
    });
}

/// A heading's buttons and choice inputs, laid out from the end of a
/// right-to-left `ui`.
fn controls(ui: &mut Ui, node: &ViewNode, page: bool, kit: &mut Kit) {
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
}

/// How much farther a section's title sits from the rows above it than
/// from its own, so each section reads as one group.
pub(super) const SECTION_GAP: f32 = 8.0;

/// Whether `node` is a section's title in a page or a panel.
pub(super) fn is_section(node: &ViewNode, kit: &Kit) -> bool {
    matches!(
        node.kind,
        ViewKind::Heading {
            level: HeadingLevel::Section,
            ..
        }
    ) && !kit.in_popover
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
    // The title names it, so the group adds no name of its own.
    let builder = UiBuilder::new().accessibility_role(Role::Group);
    ui.scope_builder(builder, |ui| {
        let controls = (!node.children.is_empty()).then_some(|ui: &mut Ui| {
            for child in &node.children {
                if let ViewKind::Button { label } = &child.kind {
                    ui.push_id(&child.id, |ui| {
                        styled_button(ui, child, label, Look::Primary, kit);
                    });
                }
            }
        });
        crate::common::empty_state_with(ui, symbol.map(icon), title, detail.as_deref(), controls);
    });
}
