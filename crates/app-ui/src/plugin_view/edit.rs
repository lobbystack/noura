//! Compact controls a details pane lines up: text edited in place, a date
//! picker and a picker, as a task's title, notes, due date, status and
//! priority use.
//!
//! Editable text reads as plain text until you click or tab into it. A
//! title, a line or a body sends its text when you leave it, a title and a
//! line also on Return, and an add field sends on Return, then empties
//! itself and keeps the keyboard. Escape puts back what the plugin drew. What you type
//! stays until the plugin draws another value, so a plugin that ignores a
//! change doesn't wipe your typing.
//!
//! The date picker and the picker are small buttons that show their value
//! and open a popup: a month with Today, Tomorrow and Next week, or a menu
//! of the options with a check by the chosen one. A pick sends `changed`.

use app_core::{
    EditableStyle, Field, InputValue, PickerOption, PluginColor, PluginEvent, ViewNode,
};
use egui::{
    Align2, Color32, FontId, Frame, Key, Margin, Popup, PopupCloseBehavior, PopupKind, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui, Vec2, WidgetInfo,
    accesskit::{HasPopup, Role},
    pos2, vec2,
};
use jiff::{ToSpan, civil::Date};

use super::Kit;
use crate::{
    common::{self, Btn, Size, Variant},
    dates::DateNames,
    icons,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, TITLE_SIZE, Weight, font, oklch, radius},
};

/// What editable text remembers between frames, in egui's memory under
/// its ID.
#[derive(Clone)]
struct Draft {
    /// The value the plugin drew last.
    drawn: String,
    text: String,
    /// Whether the plugin drew it with `focus` last time, so only a new
    /// `focus` takes the keyboard.
    focus: bool,
}

/// Text you edit in place, drawn as `style` says.
pub(super) fn editable(
    ui: &mut Ui,
    node: &ViewNode,
    (label, value, style, focus): (&str, &str, EditableStyle, bool),
    kit: &mut Kit,
) {
    let id = kit.id(&node.id);
    let held: Option<Draft> = ui.data(|data| data.get_temp(id));
    let take_focus = focus && !held.as_ref().is_some_and(|draft| draft.focus);
    let mut draft = held
        .filter(|draft| draft.drawn == value)
        .unwrap_or_else(|| Draft {
            drawn: value.to_owned(),
            text: value.to_owned(),
            focus,
        });
    draft.focus = focus;
    let palette = Palette::of(ui.ctx());
    let field_id = id.with("text");
    // The fill goes under the text, so it takes its place before the field.
    let fill = ui.painter().add(Shape::Noop);
    let (field, margin) = match style {
        EditableStyle::Title => (
            TextEdit::singleline(&mut draft.text).font(font(
                ui.ctx(),
                TITLE_SIZE + 2.0,
                Weight::Semibold,
            )),
            Margin::symmetric(6, 4),
        ),
        EditableStyle::Line => (
            TextEdit::singleline(&mut draft.text).font(FontId::proportional(BODY_SIZE)),
            Margin::symmetric(6, 4),
        ),
        EditableStyle::Body => (
            TextEdit::multiline(&mut draft.text)
                .font(FontId::proportional(BODY_SIZE))
                .desired_rows(3)
                .lock_focus(false),
            Margin::symmetric(6, 6),
        ),
        EditableStyle::Add => (
            TextEdit::singleline(&mut draft.text).font(FontId::proportional(BODY_SIZE)),
            Margin {
                left: 32,
                right: 12,
                top: 8,
                bottom: 8,
            },
        ),
    };
    let response = ui
        .add(
            field
                .id(field_id)
                .frame(Frame::NONE.inner_margin(margin))
                .hint_text(label)
                .desired_width(f32::INFINITY),
        )
        .accessible_name(label);
    if take_focus {
        response.request_focus();
    }
    let rect = response.rect;
    let shape = match style {
        EditableStyle::Add => Shape::rect_filled(rect, radius(0.8), palette.muted),
        _ if response.has_focus() => Shape::rect_stroke(
            rect,
            radius(0.6),
            Stroke::new(1.0, palette.ring),
            StrokeKind::Inside,
        ),
        _ if response.hovered() => Shape::rect_filled(rect, radius(0.6), palette.accent),
        _ => Shape::Noop,
    };
    ui.painter().set(fill, shape);
    if style == EditableStyle::Add {
        let icon =
            Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), Vec2::splat(16.0));
        icons::paint(
            ui.painter(),
            icons::PLUS,
            icon,
            palette.muted_foreground,
            false,
        );
    }
    let escaped = ui.input(|input| input.key_pressed(Key::Escape));
    if response.lost_focus() {
        // Escape puts the text back, so it doesn't also close the popover
        // around it.
        kit.escape_taken |= escaped;
        let sent = match style {
            _ if escaped && style != EditableStyle::Add => {
                draft.text = value.to_owned();
                None
            }
            EditableStyle::Add if escaped => {
                draft.text.clear();
                None
            }
            EditableStyle::Add => {
                let entered = ui.input(|input| input.key_pressed(Key::Enter));
                let text = draft.text.trim().to_owned();
                if entered {
                    draft.text.clear();
                    // The keyboard stays for the next one.
                    response.request_focus();
                }
                (entered && !text.is_empty()).then_some(InputValue::Text(text))
            }
            _ => left(style, &mut draft.text, value),
        };
        if let Some(value) = sent {
            kit.events.push(PluginEvent::Changed(Field {
                node: node.id.clone(),
                value,
            }));
        }
    }
    ui.data_mut(|data| data.insert_temp(id, draft));
}

/// What a title, a line or a body sends when you leave it holding `text`,
/// tidied as it keeps it, after the plugin drew `value`.
fn left(style: EditableStyle, text: &mut String, value: &str) -> Option<InputValue> {
    match style {
        EditableStyle::Title => {
            let trimmed = text.trim().to_owned();
            if trimmed.is_empty() {
                // A title of only spaces names nothing, so the old one
                // comes back.
                value.clone_into(text);
                None
            } else {
                text.clone_from(&trimmed);
                (trimmed != value).then_some(InputValue::Text(trimmed))
            }
        }
        EditableStyle::Line => {
            let trimmed = text.trim().to_owned();
            text.clone_from(&trimmed);
            (trimmed != value).then_some(InputValue::Text(trimmed))
        }
        EditableStyle::Body => (*text != value).then(|| InputValue::Multiline(text.clone())),
        EditableStyle::Add => None,
    }
}

/// The change the editable text `node` would send if you left it now,
/// while it has the keyboard, as when a click outside closes the popover
/// it sits in before it can.
pub(super) fn pending(ui: &Ui, node: &ViewNode, kit: &Kit) -> Option<PluginEvent> {
    let app_core::ViewKind::Editable { value, style, .. } = &node.kind else {
        return None;
    };
    let id = kit.id(&node.id);
    if !ui.memory(|memory| memory.has_focus(id.with("text"))) {
        return None;
    }
    let mut draft: Draft = ui.data(|data| data.get_temp(id))?;
    if draft.drawn != *value {
        return None;
    }
    let value = left(*style, &mut draft.text, value)?;
    ui.data_mut(|data| data.insert_temp(id, draft));
    Some(PluginEvent::Changed(Field {
        node: node.id.clone(),
        value,
    }))
}

/// What a small button that opens a popup shows before its text.
enum Mark {
    Icon(icons::Icon),
    /// A flag in this color.
    Flag(Color32),
}

/// A small button that shows `text`, muted while it stands for no value,
/// after `mark`, with a caret when it opens a menu.
fn chip(ui: &mut Ui, mark: Option<&Mark>, text: &str, muted: bool, caret: bool) -> Response {
    let palette = Palette::of(ui.ctx());
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        font(ui.ctx(), BODY_SIZE, Weight::Medium),
        Color32::PLACEHOLDER,
    );
    let lead = if mark.is_some() { 20.0 } else { 0.0 };
    let trail = if caret { 16.0 } else { 0.0 };
    let size = vec2(8.0 + lead + galley.size().x + trail + 8.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() || response.has_focus() {
            painter.rect_filled(rect, radius(0.6), palette.accent);
        }
        if response.has_focus() && common::focus_visible(ui.ctx()) {
            common::focus_ring(painter, rect, radius(0.6), &palette);
        }
        let color = if muted {
            palette.muted_foreground
        } else {
            palette.foreground
        };
        let mut x = rect.left() + 8.0;
        if let Some(mark) = mark {
            let square = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
            match mark {
                Mark::Icon(icon) => icons::paint(painter, *icon, square, color, false),
                Mark::Flag(flag) => paint_flag(painter, square, *flag),
            }
            x += lead;
        }
        let at = Align2::LEFT_CENTER.anchor_size(pos2(x, rect.center().y), galley.size());
        painter.galley(at.min, galley, color);
        if caret {
            let square = Rect::from_center_size(
                pos2(rect.right() - 8.0 - 6.0, rect.center().y),
                Vec2::splat(12.0),
            );
            icons::paint(
                painter,
                icons::CARET_DOWN,
                square,
                palette.muted_foreground,
                false,
            );
        }
    }
    response
}

/// A flag on a pole, filling `square`, in `color`.
fn paint_flag(painter: &egui::Painter, square: Rect, color: Color32) {
    let left = square.left() + square.width() * 0.2;
    let top = square.top() + 1.0;
    painter.line_segment(
        [pos2(left, top), pos2(left, square.bottom())],
        Stroke::new(1.5, color),
    );
    let right = square.right() - 1.0;
    let middle = top + square.height() * 0.32;
    let low = top + square.height() * 0.62;
    painter.add(Shape::convex_polygon(
        vec![
            pos2(left, top),
            pos2(right, top),
            pos2(right - 3.0, middle),
            pos2(right, low),
            pos2(left, low),
        ],
        color,
        Stroke::NONE,
    ));
}

/// A color of Noura's palette, for the light or dark theme.
fn named(color: PluginColor, dark: bool) -> Color32 {
    let (lightness, chroma) = if dark { (0.72, 0.15) } else { (0.62, 0.17) };
    match color {
        PluginColor::Red => oklch(lightness, chroma, 25.0),
        PluginColor::Orange => oklch(lightness, chroma, 55.0),
        PluginColor::Yellow => oklch(lightness + 0.1, chroma, 90.0),
        PluginColor::Green => oklch(lightness, chroma, 150.0),
        PluginColor::Blue => oklch(lightness, chroma, 255.0),
        PluginColor::Purple => oklch(lightness, chroma, 300.0),
        PluginColor::Brown => oklch(lightness - 0.1, chroma * 0.5, 55.0),
        PluginColor::Gray => oklch(lightness, 0.0, 0.0),
    }
}

/// Say to screen readers that the button behind `response` is named
/// `label`, shows `value` and opens a popup of `kind`, open or not.
fn describe(
    ui: &Ui,
    response: &Response,
    (label, value): (&str, &str),
    kind: HasPopup,
    open: bool,
) {
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, label));
    let value = value.to_owned();
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_value(value);
        node.set_has_popup(kind);
        node.set_expanded(open);
    });
}

/// How a date reads on the button: Today, Tomorrow or Yesterday, a
/// weekday within the week ahead, or a short date, with the year when it
/// isn't this year's.
fn date_words(day: Date, today: Date) -> String {
    match (day - today).get_days() {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        -1 => "Yesterday".to_owned(),
        2..7 => day.strftime("%A").to_string(),
        _ if day.year() == today.year() => day.strftime("%b %-d").to_string(),
        _ => day.strftime("%b %-d, %Y").to_string(),
    }
}

/// The first of `day`'s month.
fn month_of(day: Date) -> Date {
    day.first_of_month()
}

/// A button that shows a date and opens a date picker.
pub(super) fn date_picker(
    ui: &mut Ui,
    node: &ViewNode,
    label: &str,
    value: Option<&str>,
    kit: &mut Kit,
) {
    let id = kit.id(&node.id);
    let today = kit.app.today();
    let chosen: Option<Date> = value.and_then(|value| value.parse().ok());
    let words = chosen.map(|day| date_words(day, today));
    // The month the open picker shows, or none while it's closed.
    let mut shown: Option<Date> = ui.data(|data| data.get_temp(id));
    kit.popup_open |= shown.is_some();
    let response = chip(
        ui,
        Some(&Mark::Icon(icons::CALENDAR_BLANK)),
        words.as_deref().unwrap_or(label),
        chosen.is_none(),
        false,
    );
    if response.clicked() {
        shown = match shown {
            Some(_) => None,
            None => Some(month_of(chosen.unwrap_or(today))),
        };
    }
    describe(
        ui,
        &response,
        (label, words.as_deref().unwrap_or_default()),
        HasPopup::Dialog,
        shown.is_some(),
    );
    let Some(mut month) = shown else {
        ui.data_mut(|data| data.remove::<Date>(id));
        return;
    };
    let names = DateNames::of(kit.app.locale().as_deref());
    let mut open = true;
    let mut picked: Option<Option<Date>> = None;
    Popup::from_response(&response)
        .id(id.with("popup"))
        .kind(PopupKind::Popup)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .frame(common::menu_frame(ui.ctx()))
        .open_bool(&mut open)
        .show(|ui| {
            ui.set_width(7.0 * CELL.x);
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            ui.horizontal(|ui| {
                for (words, day) in [
                    ("Today", today),
                    ("Tomorrow", today.tomorrow().unwrap_or(today)),
                    ("Next week", today.checked_add(7.days()).unwrap_or(today)),
                ] {
                    let button = Btn::new(words).variant(Variant::Ghost).size(Size::Sm);
                    if button.show(ui).clicked() {
                        picked = Some(Some(day));
                    }
                }
            });
            ui.horizontal(|ui| {
                let back = Btn::icon_only(icons::CARET_LEFT, "Previous month").size(Size::Sm);
                if back.show(ui).clicked() {
                    month = month.checked_sub(1.month()).unwrap_or(month);
                }
                let title = names.month(month);
                let width = ui.available_width() - 28.0 - ui.spacing().item_spacing.x;
                ui.allocate_ui(vec2(width, 28.0), |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label(egui::RichText::new(title).font(font(
                            ui.ctx(),
                            BODY_SIZE,
                            Weight::Medium,
                        )));
                    });
                });
                let next = Btn::icon_only(icons::CARET_RIGHT, "Next month").size(Size::Sm);
                if next.show(ui).clicked() {
                    month = month.checked_add(1.month()).unwrap_or(month);
                }
            });
            if let Some(day) = month_grid(ui, month, (chosen, today), &names) {
                picked = Some(Some(day));
            }
            if chosen.is_some() {
                let clear = Btn::new("Clear").variant(Variant::Ghost).size(Size::Sm);
                if clear.show(ui).clicked() {
                    picked = Some(None);
                }
            }
        });
    if let Some(day) = picked {
        open = false;
        if day != chosen {
            kit.events.push(PluginEvent::Changed(Field {
                node: node.id.clone(),
                value: InputValue::Date(day.map(|day| day.to_string())),
            }));
        }
    }
    if open {
        ui.data_mut(|data| data.insert_temp(id, month));
    } else {
        ui.data_mut(|data| data.remove::<Date>(id));
        response.request_focus();
    }
}

/// The size of a day in the picker's month.
const CELL: Vec2 = vec2(34.0, 30.0);

/// Six weeks from the Sunday on or before the first of `month`, with
/// `chosen` filled and `today` marked. Returns the day picked.
fn month_grid(
    ui: &mut Ui,
    month: Date,
    (chosen, today): (Option<Date>, Date),
    names: &DateNames,
) -> Option<Date> {
    let palette = Palette::of(ui.ctx());
    let back = i64::from(month.weekday().to_sunday_zero_offset());
    let start = month.checked_sub(back.days()).unwrap_or(month);
    let mut picked = None;
    ui.spacing_mut().item_spacing = Vec2::ZERO;
    ui.horizontal(|ui| {
        for offset in 0..7 {
            let day = start.checked_add(offset.days()).unwrap_or(start);
            let name: String = names.weekday(day).chars().take(2).collect();
            let (rect, _) = ui.allocate_exact_size(vec2(CELL.x, 20.0), Sense::hover());
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                name,
                FontId::proportional(SUPPORTING_SIZE),
                palette.muted_foreground,
            );
        }
    });
    for week in 0..6 {
        ui.horizontal(|ui| {
            for weekday in 0..7 {
                let day = start
                    .checked_add((week * 7 + weekday).days())
                    .unwrap_or(start);
                let (rect, response) = ui.allocate_exact_size(CELL, Sense::click());
                let full = names.full_date(day);
                response.widget_info(|| {
                    WidgetInfo::selected(Role::Button, true, Some(day) == chosen, &full)
                });
                if response.clicked() {
                    picked = Some(day);
                }
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let painter = ui.painter();
                let circle = Rect::from_center_size(rect.center(), Vec2::splat(26.0));
                let mut color = if day.month() == month.month() {
                    palette.foreground
                } else {
                    palette.muted_foreground
                };
                if Some(day) == chosen {
                    painter.rect_filled(circle, radius(1.3), palette.primary);
                    color = palette.primary_foreground;
                } else if response.hovered() || response.has_focus() {
                    painter.rect_filled(circle, radius(1.3), palette.accent);
                }
                let weight = if day == today {
                    Weight::Semibold
                } else {
                    Weight::Regular
                };
                if day == today && Some(day) != chosen {
                    color = palette.destructive;
                }
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    day.day().to_string(),
                    font(ui.ctx(), BODY_SIZE, weight),
                    color,
                );
            }
        });
    }
    picked
}

/// A button that shows the chosen option and opens a menu of `options`.
pub(super) fn picker(
    ui: &mut Ui,
    node: &ViewNode,
    label: &str,
    (options, chosen): (&[PickerOption], Option<&str>),
    kit: &mut Kit,
) {
    let id = kit.id(&node.id);
    let dark = ui.visuals().dark_mode;
    let current = options
        .iter()
        .find(|option| Some(option.id.as_str()) == chosen);
    let mark = current
        .and_then(|option| option.color)
        .map(|color| Mark::Flag(named(color, dark)));
    let text = current.map_or(label, |option| option.label.as_str());
    let mut open: bool = ui.data(|data| data.get_temp(id)).unwrap_or(false);
    kit.popup_open |= open;
    let response = chip(ui, mark.as_ref(), text, current.is_none(), true);
    let opening = response.clicked() && !open;
    if response.clicked() {
        open = !open;
    }
    describe(
        ui,
        &response,
        (label, current.map_or("", |option| option.label.as_str())),
        HasPopup::Menu,
        open,
    );
    if !open {
        ui.data_mut(|data| data.remove::<bool>(id));
        return;
    }
    let mut picked = None;
    Popup::from_response(&response)
        .id(id.with("menu"))
        .kind(PopupKind::Menu)
        .style(crate::theme::menu_style)
        .frame(common::menu_frame(ui.ctx()))
        .open_bool(&mut open)
        .accessible_name(label.to_owned())
        .width(180.0)
        .show(|ui| {
            ui.set_width(180.0);
            // A long menu, such as the times of a day, scrolls, and opens
            // on the chosen option.
            egui::ScrollArea::vertical()
                .max_height(MENU_HEIGHT)
                .show(ui, |ui| {
                    for option in options {
                        let on = Some(option.id.as_str()) == chosen;
                        let row = option_row(ui, option, on, dark);
                        if on && opening {
                            row.scroll_to_me(Some(egui::Align::Center));
                        }
                        if row.clicked() {
                            picked = Some(option.id.clone());
                        }
                    }
                });
        });
    if let Some(option) = picked {
        open = false;
        if Some(option.as_str()) != chosen {
            kit.events.push(PluginEvent::Changed(Field {
                node: node.id.clone(),
                value: InputValue::Choice(Some(option)),
            }));
        }
    }
    if open {
        ui.data_mut(|data| data.insert_temp(id, true));
    } else {
        ui.data_mut(|data| data.remove::<bool>(id));
        response.request_focus();
    }
}

/// The tallest a picker's menu grows before it scrolls.
const MENU_HEIGHT: f32 = 320.0;

/// An option in a picker's menu: its mark, its label, and a check when
/// it's the chosen one.
fn option_row(ui: &mut Ui, option: &PickerOption, on: bool, dark: bool) -> Response {
    let palette = Palette::of(ui.ctx());
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::MenuItemRadio, true, on, &option.label));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() || response.has_focus() {
            painter.rect_filled(rect, radius(0.6), palette.accent);
        }
        let mut x = rect.left() + 8.0;
        if let Some(color) = option.color {
            let square = Rect::from_center_size(pos2(x + 7.0, rect.center().y), Vec2::splat(14.0));
            paint_flag(painter, square, named(color, dark));
            x += 22.0;
        }
        painter.text(
            pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            &option.label,
            FontId::proportional(BODY_SIZE),
            palette.foreground,
        );
        if on {
            let square = Rect::from_center_size(
                pos2(rect.right() - 14.0, rect.center().y),
                Vec2::splat(14.0),
            );
            icons::paint(painter, icons::CHECK, square, palette.foreground, false);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::date_words;

    #[test]
    fn a_date_reads_as_a_word_near_today_and_a_short_date_further_off() {
        let today = date(2026, 11, 21);
        assert_eq!(date_words(today, today), "Today");
        assert_eq!(date_words(date(2026, 11, 22), today), "Tomorrow");
        assert_eq!(date_words(date(2026, 11, 20), today), "Yesterday");
        assert_eq!(date_words(date(2026, 11, 24), today), "Tuesday");
        assert_eq!(date_words(date(2026, 12, 25), today), "Dec 25");
        assert_eq!(date_words(date(2027, 1, 2), today), "Jan 2, 2027");
    }
}
