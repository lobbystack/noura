//! The command palette: a dialog with a field that filters every command
//! as you type and a list of the matches with their shortcuts. `app-core`
//! keeps the query, the matches and the highlighted row; Enter runs the
//! highlighted command through the same dispatch as keys and menus.

use std::time::Instant;

use app_core::App;
use egui::{
    Align2, Event, EventFilter, Frame, Id, Key, Margin, Modal, RichText, ScrollArea, Sense,
    TextEdit, Ui, UiBuilder, WidgetInfo,
    accesskit::{Action, Role},
    vec2,
};

use crate::{
    keys,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, radius},
};

const WIDTH: f32 = 520.0;
const ROW_HEIGHT: f32 = 32.0;
const LIST_HEIGHT: f32 = 320.0;

/// What the palette remembers between frames.
#[derive(Default)]
pub(crate) struct PaletteView {
    /// Whether the palette showed last frame.
    shown: bool,
    /// The widget that had the keyboard before the palette opened.
    return_focus: Option<Id>,
    /// Scroll the highlighted row into view on the next frame.
    reveal: bool,
}

fn field_id() -> Id {
    Id::unique("command-palette-field")
}

fn row_id(index: usize) -> Id {
    Id::unique(("command-palette-row", index))
}

/// The keys the palette takes from its field: Up and Down move the
/// highlight, Enter runs it and Escape closes the palette.
enum PaletteKey {
    Move(isize),
    Run,
    Close,
}

fn take_keys(ui: &Ui) -> Vec<PaletteKey> {
    ui.input_mut(|input| {
        let mut taken = Vec::new();
        input.events.retain(|event| {
            let Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            if modifiers.any() {
                return true;
            }
            let found = match key {
                Key::ArrowDown => PaletteKey::Move(1),
                Key::ArrowUp => PaletteKey::Move(-1),
                Key::Enter => PaletteKey::Run,
                Key::Escape => PaletteKey::Close,
                _ => return true,
            };
            taken.push(found);
            false
        });
        taken
    })
}

/// Draw the palette while it's open, and give the keyboard back to where
/// it was once it closes.
pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut PaletteView) {
    let ctx = ui.ctx().clone();
    let Some(palette) = app.palette().cloned() else {
        if std::mem::take(&mut view.shown)
            && let Some(id) = view.return_focus.take()
        {
            ctx.memory_mut(|memory| memory.request_focus(id));
        }
        return;
    };
    let opening = !view.shown;
    if opening {
        view.shown = true;
        view.return_focus = ctx.memory(|memory| memory.focused());
    }

    let platform = app.platform();
    let theme = Palette::of(&ctx);
    let mut close = false;
    let mut run = false;
    let mut chosen = None;
    let area = Modal::default_area(Id::unique("command-palette"))
        .anchor(Align2::CENTER_TOP, vec2(0.0, 72.0));
    let modal = Modal::new(Id::unique("command-palette"))
        .area(area)
        .frame(
            Frame::popup(&ctx.global_style())
                .fill(theme.popover)
                .corner_radius(radius(1.4))
                .inner_margin(Margin::same(8)),
        )
        .show(&ctx, |ui| {
            ui.set_width(WIDTH);
            let builder = UiBuilder::new()
                .accessibility_role(Role::Dialog)
                .accessibility_label("Command palette");
            ui.scope_builder(builder, |ui| {
                for key in take_keys(ui) {
                    match key {
                        PaletteKey::Move(offset) => {
                            app.move_palette_selection(offset);
                            view.reveal = true;
                        }
                        PaletteKey::Run => run = true,
                        PaletteKey::Close => close = true,
                    }
                }
                let mut query = palette.query.clone();
                let field = ui.add(
                    TextEdit::singleline(&mut query)
                        .id(field_id())
                        .hint_text("Type a command")
                        .desired_width(f32::INFINITY),
                );
                let field = field.accessible_name("Search commands");
                if opening {
                    field.request_focus();
                }
                // Up and Down move the highlight, not the keyboard focus.
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        field.id,
                        EventFilter {
                            tab: false,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: false,
                        },
                    );
                });
                if field.changed() {
                    app.set_palette_query(&query);
                }
                ui.add_space(4.0);
                chosen = rows(ui, app, view, platform, &theme);
            });
        });
    if modal.should_close() {
        close = true;
    }

    if let Some(row) = chosen {
        app.select_palette_row(row);
        run = true;
    }
    if run {
        app.run_palette_selection(Instant::now());
    } else if close {
        app.close_palette();
    }
    if app.palette().is_none() {
        // Give the keyboard back now, so the next frame's keys go there.
        view.shown = false;
        match view.return_focus.take() {
            Some(id) => ctx.memory_mut(|memory| memory.request_focus(id)),
            None => ctx.memory_mut(|memory| memory.surrender_focus(field_id())),
        }
    }
}

/// The matching commands, each with its shortcut. Returns the row you
/// clicked.
fn rows(
    ui: &mut Ui,
    app: &App,
    view: &mut PaletteView,
    platform: app_core::Platform,
    theme: &Palette,
) -> Option<usize> {
    let matches = app.palette_matches();
    let selected = app.palette().map_or(0, |palette| palette.selected);
    if matches.is_empty() {
        let query = app.palette().map(|palette| palette.query.as_str());
        let text = format!("No commands match \u{201c}{}\u{201d}.", query.unwrap_or(""));
        ui.label(
            RichText::new(text)
                .size(SUPPORTING_SIZE)
                .color(theme.muted_foreground),
        );
        return None;
    }
    let mut clicked = None;
    let builder = UiBuilder::new()
        .accessibility_role(Role::ListBox)
        .accessibility_label("Commands");
    ui.scope_builder(builder, |ui| {
        ScrollArea::vertical()
            .id_salt("command-palette-list")
            .max_height(LIST_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (index, command) in matches.iter().enumerate() {
                    let keys = command
                        .shortcuts
                        .first()
                        .map(|shortcut| keys::shortcut_text(shortcut, platform));
                    let (rect, _) = ui.allocate_exact_size(
                        vec2(ui.available_width(), ROW_HEIGHT),
                        Sense::hover(),
                    );
                    let id = row_id(index);
                    let response = ui.interact(rect, id, Sense::CLICK);
                    let is_selected = index == selected;
                    response.widget_info(|| {
                        WidgetInfo::labeled(Role::ListBoxOption, true, &command.label)
                    });
                    ui.ctx().accesskit_node_builder(id, |node| {
                        node.set_selected(is_selected);
                        node.add_action(Action::Click);
                        if let Some(keys) = &keys {
                            node.set_keyboard_shortcut(keys.clone());
                            node.set_description(keys.clone());
                        }
                    });
                    if is_selected && std::mem::take(&mut view.reveal) {
                        response.scroll_to_me(None);
                    }
                    if response.clicked() {
                        clicked = Some(index);
                    }
                    if ui.is_rect_visible(rect) {
                        let painter = ui.painter();
                        if is_selected || response.hovered() {
                            painter.rect_filled(rect, radius(0.6), theme.accent);
                        }
                        painter.text(
                            rect.left_center() + vec2(10.0, 0.0),
                            Align2::LEFT_CENTER,
                            &command.label,
                            egui::FontId::proportional(BODY_SIZE),
                            theme.foreground,
                        );
                        if let Some(keys) = &keys {
                            painter.text(
                                rect.right_center() - vec2(10.0, 0.0),
                                Align2::RIGHT_CENTER,
                                keys,
                                egui::FontId::proportional(SUPPORTING_SIZE),
                                theme.muted_foreground,
                            );
                        }
                    }
                }
            });
    });
    clicked
}
