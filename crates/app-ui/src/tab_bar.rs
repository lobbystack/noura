//! The tab bar: one tab per open file or page, each with a close button.
//! `app-core` owns the tabs; this draws them and sends it clicks and keys.

use app_core::{App, TabId};
use egui::{
    Align2, Event, EventFilter, FontId, Id, Key, Rect, Response, ScrollArea, Sense, Stroke,
    StrokeKind, TextWrapMode, Ui, UiBuilder, WidgetInfo, WidgetText,
    accesskit::{Action, Role},
    containers::scroll_area::ScrollBarVisibility,
    pos2, vec2,
};

use crate::theme::{BODY_SIZE, Palette, radius};

/// The height of the tab bar.
pub(crate) const BAR_HEIGHT: f32 = 40.0;
const TAB_HEIGHT: f32 = 28.0;
/// The widest a tab's name draws before it's cut short.
const NAME_WIDTH: f32 = 160.0;
const CLOSE_SIZE: f32 = 16.0;

/// What the tab bar remembers between frames.
#[derive(Default)]
pub(crate) struct TabBarView {
    /// The tab that takes Tab focus, after arrow keys move it.
    focus: Option<TabId>,
    /// Give `focus` keyboard focus on the next frame.
    grab: bool,
}

fn tab_id(tab: TabId) -> Id {
    Id::unique(("tab", tab))
}

/// Whether one of the tabs has keyboard focus.
pub(crate) fn has_focus(ctx: &egui::Context, app: &App) -> bool {
    ctx.memory(|memory| memory.focused())
        .is_some_and(|focused| app.tabs().iter().any(|tab| tab_id(tab.id()) == focused))
}

/// Keys the bar takes while one of its tabs has focus.
const KEY_FILTER: EventFilter = EventFilter {
    tab: false,
    horizontal_arrows: true,
    vertical_arrows: false,
    escape: false,
};

enum Move {
    To(usize),
    Close,
}

/// Take the bar's keys out of this frame's input: arrows, Home and End
/// move between tabs, and Delete or Backspace close the focused one.
fn take_key(ui: &Ui, index: usize, count: usize) -> Option<Move> {
    let last = count - 1;
    ui.input_mut(|input| {
        let mut found = None;
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
            let action = match key {
                Key::ArrowRight => Move::To(if index == last { 0 } else { index + 1 }),
                Key::ArrowLeft => Move::To(if index == 0 { last } else { index - 1 }),
                Key::Home => Move::To(0),
                Key::End => Move::To(last),
                Key::Delete | Key::Backspace => Move::Close,
                _ => return true,
            };
            found = Some(action);
            false
        });
        found
    })
}

struct Shown {
    id: TabId,
    title: String,
    preview: bool,
}

/// Draw the tab bar's tabs in the rest of `ui`'s row.
pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut TabBarView) {
    let tabs: Vec<Shown> = app
        .tabs()
        .iter()
        .map(|tab| Shown {
            id: tab.id(),
            title: tab.title().to_owned(),
            preview: tab.is_preview(),
        })
        .collect();
    if tabs.is_empty() {
        *view = TabBarView::default();
        return;
    }
    let active = app.active_tab().map(app_core::Tab::id);
    if view
        .focus
        .is_some_and(|focus| !tabs.iter().any(|tab| tab.id == focus))
    {
        view.focus = None;
    }

    // A tab has keyboard focus when its own widget does, or when a screen
    // reader asks to focus it.
    let focused = ui.input(|input| {
        tabs.iter()
            .position(|tab| input.has_accesskit_action_request(tab_id(tab.id), Action::Focus))
    });
    let focused = focused.or_else(|| {
        let focused = ui.memory(|memory| memory.focused())?;
        tabs.iter().position(|tab| tab_id(tab.id) == focused)
    });
    if focused.is_none() && !view.grab {
        // The keyboard left the bar; Tab comes back to the active tab.
        view.focus = None;
    }
    let mut close = None;
    if let Some(index) = focused {
        view.focus = Some(tabs[index].id);
        match take_key(ui, index, tabs.len()) {
            Some(Move::To(target)) => {
                view.focus = Some(tabs[target].id);
                view.grab = true;
            }
            Some(Move::Close) => close = Some((index, true)),
            None => {}
        }
    }
    // Like the legacy tab bar, only one tab takes Tab focus: the focused
    // one, else the active one, else the first.
    let tab_stop = view.focus.or(active).unwrap_or(tabs[0].id);

    let mut activate = None;
    let mut keep = None;
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::TabList)
        .accessibility_label("Tabs");
    ui.scope_builder(builder, |ui| {
        // A mouse wheel moves the tabs sideways, as the bar scrolls no
        // other way.
        ui.style_mut().always_scroll_the_only_direction = true;
        ScrollArea::horizontal()
            .id_salt("tab-bar")
            .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (index, tab) in tabs.iter().enumerate() {
                        let is_tab_stop = tab.id == tab_stop;
                        let drawn =
                            draw_tab(ui, tab, Some(tab.id) == active, is_tab_stop, &palette);
                        if view.grab && Some(tab.id) == view.focus {
                            drawn.tab.request_focus();
                            drawn.tab.scroll_to_me(None);
                        }
                        if drawn.tab.has_focus() {
                            let id = drawn.tab.id;
                            ui.memory_mut(|memory| memory.set_focus_lock_filter(id, KEY_FILTER));
                        }
                        if drawn.tab.middle_clicked() || drawn.close.clicked() {
                            close = Some((index, false));
                        } else if drawn.tab.clicked() {
                            activate = Some(tab.id);
                            if drawn.tab.double_clicked() {
                                keep = Some(tab.id);
                            }
                        }
                    }
                });
            });
    });
    view.grab = false;

    if let Some(id) = activate {
        app.activate_tab(id);
    }
    if let Some(id) = keep {
        app.keep_tab(id);
    }
    if let Some((index, from_keyboard)) = close {
        app.close_tab(tabs[index].id);
        // The keyboard stays in the bar, on the tab that takes the closed
        // one's place.
        let remaining = app.tabs();
        view.focus = remaining
            .get(index.min(remaining.len().saturating_sub(1)))
            .map(app_core::Tab::id);
        view.grab = from_keyboard && view.focus.is_some();
    }
}

struct DrawnTab {
    tab: Response,
    close: Response,
}

/// One tab: its name, italic while it's the preview, and a close button
/// that shows on the active tab and under the pointer.
fn draw_tab(ui: &mut Ui, tab: &Shown, active: bool, tab_stop: bool, palette: &Palette) -> DrawnTab {
    let mut name = egui::RichText::new(&tab.title);
    if tab.preview {
        name = name.italics();
    }
    let galley = WidgetText::from(name).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        NAME_WIDTH,
        FontId::proportional(BODY_SIZE),
    );
    let size = vec2(10.0 + galley.size().x + 6.0 + CLOSE_SIZE + 6.0, TAB_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let sense = if tab_stop {
        Sense::click()
    } else {
        Sense::CLICK
    };
    let id = tab_id(tab.id);
    let label = if tab.preview {
        format!("{} (preview)", tab.title)
    } else {
        tab.title.clone()
    };
    let response = ui.interact(rect, id, sense);
    response.widget_info(|| WidgetInfo::labeled(Role::Tab, true, &label));
    ui.ctx().accesskit_node_builder(id, |node| {
        node.add_action(Action::Focus);
        node.set_selected(active);
    });
    let response = response.on_hover_text(&label);

    let close_rect = Rect::from_center_size(
        pos2(rect.right() - 6.0 - CLOSE_SIZE / 2.0, rect.center().y),
        vec2(CLOSE_SIZE, CLOSE_SIZE),
    );
    let close = ui.interact(close_rect, id.with("close"), Sense::CLICK);
    let close_label = format!("Close {}", tab.title);
    close.widget_info(|| WidgetInfo::labeled(Role::Button, true, &close_label));

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hovered = response.hovered() || close.hovered();
        if active || hovered {
            painter.rect_filled(rect, radius(0.8), palette.background);
        }
        if active {
            painter.rect_stroke(
                rect,
                radius(0.8),
                Stroke::new(1.0, palette.border),
                StrokeKind::Inside,
            );
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect.shrink(1.0),
                radius(0.7),
                Stroke::new(1.0, palette.ring),
                StrokeKind::Inside,
            );
        }
        let color = if active || hovered {
            palette.foreground
        } else {
            palette.muted_foreground
        };
        let text = Align2::LEFT_CENTER
            .anchor_size(pos2(rect.left() + 10.0, rect.center().y), galley.size());
        painter.galley(text.min, galley, color);
        if active || hovered {
            if close.hovered() {
                painter.rect_filled(close_rect, radius(0.4), palette.muted);
            }
            let cross = close_rect.shrink(4.5);
            let stroke = Stroke::new(1.25, palette.muted_foreground);
            painter.line_segment([cross.left_top(), cross.right_bottom()], stroke);
            painter.line_segment([cross.right_top(), cross.left_bottom()], stroke);
        }
    }
    DrawnTab {
        tab: response,
        close,
    }
}
