//! The tab bar: one tab per open file or page, each with a close button.
//! `app-core` owns the tabs; this draws them and sends it clicks and keys.
//!
//! The active tab is the top of the page under it: the page's own fill,
//! rounded at the top, open at the bottom, where the row's hairline stops
//! for it, so tab and page read as one sheet. The other tabs sit on the
//! row with no box, a faint fill under the pointer and a thin line between
//! neighbours. Tabs share the row evenly between their narrowest and widest,
//! and past their narrowest the row scrolls.
//!
//! You reorder tabs by dragging them. The row is also the window's title
//! bar, whose empty part moves the window, so a tab takes its own drags:
//! a tab that only took clicks would leave the drag to the title bar under
//! it, and the window would move.

use app_core::{App, TabId};
use egui::{
    Align2, CornerRadius, DragAndDrop, Event, EventFilter, FontId, Id, Key, Rect, Response,
    ScrollArea, Sense, Stroke, StrokeKind, TextWrapMode, Ui, UiBuilder, ViewportId, WidgetInfo,
    WidgetText,
    accesskit::{Action, Role},
    containers::scroll_area::ScrollBarVisibility,
    pos2, vec2,
};

use crate::theme::{BODY_SIZE, Palette, radius};

/// The height of the tab bar.
pub(crate) const BAR_HEIGHT: f32 = 36.0;
/// The space above a tab: tabs stand on the row's bottom edge.
const TAB_TOP: f32 = 6.0;
/// A tab's narrowest and widest. Tabs share the row evenly between them.
pub(crate) const TAB_MIN_WIDTH: f32 = 160.0;
pub(crate) const TAB_MAX_WIDTH: f32 = 240.0;
/// The space before a tab's name and after its close button.
const TAB_PADDING: f32 = 12.0;
const CLOSE_SIZE: f32 = 16.0;
/// The active tab's top corners.
const TAB_RADIUS: u8 = 8;

/// What the tab bar remembers between frames.
#[derive(Default)]
pub(crate) struct TabBarView {
    /// The tab that takes Tab focus, after arrow keys move it.
    focus: Option<TabId>,
    /// Give `focus` keyboard focus on the next frame.
    grab: bool,
    /// The tab that was active last frame: a newly active tab scrolls
    /// into sight.
    shown_active: Option<TabId>,
}

fn tab_id(tab: TabId) -> Id {
    Id::unique(("tab", tab))
}

/// The tab you're dragging to a new place, as egui's drag-and-drop
/// payload. egui drops the payload when you press Escape.
struct Dragged {
    /// The window the drag started in. Windows share the payload, and each
    /// numbers its own tabs.
    viewport: ViewportId,
    id: TabId,
    from: usize,
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

/// Draw the tab bar's tabs in `ui`'s row, up to `end`. Returns where the
/// active tab drew, which the row's hairline leaves open.
pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut TabBarView, end: f32) -> Option<Rect> {
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
        return None;
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
    let mut moved = None;
    let mut active_rect = None;
    let palette = Palette::of(ui.ctx());
    // Tabs share the row evenly, as wide as they may be, and no narrower
    // than their narrowest: past that, the row scrolls.
    let row_width = (end - ui.cursor().left()).max(0.0);
    let width = (row_width / tabs.len() as f32).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH);
    // A dragged tab drops anywhere along the row, the empty part after
    // the last tab included.
    let drop_zone = Rect::from_x_y_ranges(ui.cursor().left()..=end, ui.max_rect().y_range());
    let viewport = ui.ctx().viewport_id();
    let dragged = DragAndDrop::payload::<Dragged>(ui.ctx()).filter(|dragged| {
        dragged.viewport == viewport
            && tabs
                .get(dragged.from)
                .is_some_and(|tab| tab.id == dragged.id)
    });
    let builder = UiBuilder::new()
        .accessibility_role(Role::TabList)
        .accessibility_label("Tabs");
    ui.scope_builder(builder, |ui| {
        // A mouse wheel moves the tabs sideways, as the bar scrolls no
        // other way.
        ui.style_mut().always_scroll_the_only_direction = true;
        ScrollArea::horizontal()
            .id_salt("tab-bar")
            .max_width(row_width)
            .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let mut drawn_tabs: Vec<(Rect, bool)> = Vec::with_capacity(tabs.len());
                    for (index, tab) in tabs.iter().enumerate() {
                        let is_tab_stop = tab.id == tab_stop;
                        let is_active = Some(tab.id) == active;
                        let look = Look {
                            active: is_active,
                            tab_stop: is_tab_stop,
                            dragging: dragged.as_ref().is_some_and(|d| d.id == tab.id),
                        };
                        let drawn = draw_tab(ui, tab, width, look, &palette);
                        drawn.tab.dnd_set_drag_payload(Dragged {
                            viewport,
                            id: tab.id,
                            from: index,
                        });
                        if is_active {
                            if view.shown_active != active {
                                drawn.tab.scroll_to_me(None);
                            }
                            active_rect = Some(drawn.rect.intersect(ui.clip_rect()));
                        }
                        drawn_tabs.push((drawn.rect, is_active || drawn.hovered));
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
                    // A thin line between two quiet tabs; none beside the
                    // active tab or the one under the pointer.
                    for pair in drawn_tabs.windows(2) {
                        let [(left, left_lit), (_, right_lit)] = pair else {
                            continue;
                        };
                        if *left_lit || *right_lit {
                            continue;
                        }
                        let x = left.right();
                        let middle = (left.top() + left.bottom()) / 2.0;
                        ui.painter().vline(
                            x,
                            (middle - 8.0)..=(middle + 8.0),
                            Stroke::new(1.0, palette.border),
                        );
                    }
                    if let Some(dragged) = &dragged {
                        let rects: Vec<Rect> = drawn_tabs.iter().map(|(rect, _)| *rect).collect();
                        moved = drop_dragged(ui, dragged, &rects, drop_zone, &palette);
                    }
                });
            });
    });
    view.grab = false;
    view.shown_active = active;

    if let Some(id) = activate {
        app.activate_tab(id);
    }
    if let Some((id, index)) = moved {
        app.move_tab(id, index);
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
    active_rect
}

/// Draw the tab row's hairline along `row`'s bottom, open under the active
/// tab at `active`, so the tab and the page under it read as one sheet.
pub(crate) fn hairline(ui: &Ui, row: Rect, active: Option<Rect>) {
    let palette = Palette::of(ui.ctx());
    let y = row.bottom() - 0.5;
    let stroke = Stroke::new(1.0, palette.sidebar_border);
    let painter = ui.ctx().layer_painter(ui.layer_id()).with_clip_rect(row);
    match active.filter(|tab| tab.width() > 0.0) {
        Some(tab) => {
            painter.hline(row.left()..=tab.left(), y, stroke);
            painter.hline(tab.right()..=row.right(), y, stroke);
        }
        None => {
            painter.hline(row.x_range(), y, stroke);
        }
    }
}

/// While a tab is dragged over the row, mark the gap it would drop into
/// with a line, and on release return the tab and its new index. `tabs`
/// is where each tab drew. A gap beside the dragged tab moves nothing and
/// shows no line.
fn drop_dragged(
    ui: &Ui,
    dragged: &Dragged,
    tabs: &[Rect],
    drop_zone: Rect,
    palette: &Palette,
) -> Option<(TabId, usize)> {
    let pointer = ui.input(|input| input.pointer.latest_pos())?;
    if !drop_zone.contains(pointer) {
        return None;
    }
    // The gap after every tab whose middle is left of the pointer.
    let gap = tabs.iter().filter(|tab| tab.center().x < pointer.x).count();
    if gap == dragged.from || gap == dragged.from + 1 {
        return None;
    }
    let first = tabs.first()?;
    let x = match gap {
        0 => first.left(),
        _ => tabs.get(gap - 1)?.right(),
    };
    let clip = ui.clip_rect();
    ui.painter().vline(
        x.clamp(clip.left() + 1.0, clip.right() - 1.0),
        (first.top() + 4.0)..=(first.bottom() - 4.0),
        Stroke::new(2.0, palette.primary),
    );
    // The gap counts the dragged tab; its new index doesn't.
    let to = if gap > dragged.from { gap - 1 } else { gap };
    ui.input(|input| input.pointer.any_released())
        .then_some((dragged.id, to))
}

struct DrawnTab {
    tab: Response,
    close: Response,
    /// Where the tab drew, from its top to the row's bottom.
    rect: Rect,
    hovered: bool,
}

/// How a tab draws, besides its name.
#[derive(Clone, Copy)]
struct Look {
    active: bool,
    /// The one tab Tab focus reaches.
    tab_stop: bool,
    /// You're dragging the tab to a new place.
    dragging: bool,
}

/// One tab, `width` wide: its name, in Public Sans Italic while it's the
/// preview, and a close button that shows on the active tab and under the
/// pointer. The active tab is the page's sheet; another tab has no box.
/// Its name fades while you drag it.
fn draw_tab(ui: &mut Ui, tab: &Shown, width: f32, look: Look, palette: &Palette) -> DrawnTab {
    let Look {
        active,
        tab_stop,
        dragging,
    } = look;
    let mut name = egui::RichText::new(&tab.title);
    let mut font = FontId::proportional(BODY_SIZE);
    if tab.preview {
        let (italic, drawn) = crate::theme::italic_font(ui.ctx(), BODY_SIZE);
        font = italic;
        if !drawn {
            name = name.italics();
        }
    }
    let name_width = (width - TAB_PADDING - 6.0 - CLOSE_SIZE - TAB_PADDING / 2.0).max(0.0);
    let galley = WidgetText::from(name.font(font.clone())).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        name_width,
        font,
    );
    let (slot, _) = ui.allocate_exact_size(vec2(width, BAR_HEIGHT), Sense::hover());
    // The tab stands on the row's bottom edge.
    let rect = Rect::from_min_max(pos2(slot.left(), slot.top() + TAB_TOP), slot.max);
    // The tab takes its drags, which reorder the tabs, so the title bar
    // under it doesn't move the window.
    let sense = if tab_stop {
        Sense::click_and_drag()
    } else {
        Sense::CLICK | Sense::DRAG
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

    let middle = rect.center().y;
    let close_rect = Rect::from_center_size(
        pos2(rect.right() - TAB_PADDING / 2.0 - CLOSE_SIZE / 2.0, middle),
        vec2(CLOSE_SIZE, CLOSE_SIZE),
    );
    let close = ui.interact(close_rect, id.with("close"), Sense::CLICK);
    let close_label = format!("Close {}", tab.title);
    close.widget_info(|| WidgetInfo::labeled(Role::Button, true, &close_label));
    let hovered = response.hovered() || close.hovered();

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let top = CornerRadius {
            nw: TAB_RADIUS,
            ne: TAB_RADIUS,
            sw: 0,
            se: 0,
        };
        if active {
            // The page's own fill, outlined on three sides by the row's
            // hairline, which stops under the tab.
            painter.rect_filled(rect, top, palette.background);
            let outline = Rect::from_min_max(rect.min, rect.max + vec2(0.0, 2.0));
            painter
                .with_clip_rect(rect.intersect(ui.clip_rect()))
                .rect_stroke(
                    outline,
                    top,
                    Stroke::new(1.0, palette.sidebar_border),
                    StrokeKind::Inside,
                );
        } else if hovered {
            let fill = Rect::from_min_max(rect.min + vec2(2.0, 0.0), rect.max - vec2(2.0, 4.0));
            painter.rect_filled(fill, radius(0.8), palette.foreground.gamma_multiply(0.06));
        }
        if response.has_focus() {
            crate::common::focus_ring(painter, rect.shrink(2.0), radius(0.6), palette);
        }
        let mut color = if active || hovered {
            palette.foreground
        } else {
            palette.muted_foreground
        };
        if dragging {
            color = color.gamma_multiply(0.4);
        }
        let text =
            Align2::LEFT_CENTER.anchor_size(pos2(rect.left() + TAB_PADDING, middle), galley.size());
        painter.galley(text.min, galley, color);
        if active || hovered {
            let color = if close.hovered() {
                palette.foreground
            } else {
                palette.muted_foreground.gamma_multiply(0.6)
            };
            if close.hovered() {
                painter.rect_filled(
                    close_rect.expand(2.0),
                    radius(0.4),
                    palette.foreground.gamma_multiply(0.08),
                );
            }
            let cross = Rect::from_center_size(close_rect.center(), vec2(14.0, 14.0));
            crate::icons::paint(painter, crate::icons::CLOSE, cross, color, false);
        }
    }
    DrawnTab {
        tab: response,
        close,
        rect,
        hovered,
    }
}
