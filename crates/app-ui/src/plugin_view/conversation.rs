//! Conversations, after the chat layout of shadcn's AI Elements: a column
//! of messages in the middle of the page that scrolls on its own and keeps
//! to its end, your messages in a bubble at the end of the line and the
//! others as Markdown at the start, quiet rows for the steps taken along
//! the way that open to show what they did, and a box to write the next
//! message in, pinned under the column.
//!
//! The composer's text stays until the plugin draws the box with another
//! value or you send it, as a form's input does. Enter sends and
//! Shift-Enter starts a new line.

use app_core::{
    ActivityState, Field, InputValue, MessageAuthor, PluginEvent, Submission, ViewKind, ViewNode,
};
use egui::{
    Align, Color32, Frame, Key, KeyboardShortcut, Label, Layout, Margin, Modifiers, Rect, RichText,
    ScrollArea, Sense, Shadow, Stroke, TextEdit, Ui, UiBuilder, WidgetInfo, accesskit::Role, pos2,
    vec2,
};

use super::{Kit, form};
use crate::{
    common::{Btn, Size, Variant},
    icons, markdown,
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, Weight, font, pill, radius},
};

/// The widest a conversation's column runs, as in the chat apps it
/// follows.
const COLUMN_WIDTH: f32 = 720.0;

/// The room on each side of the column on a narrow page, and under the
/// composer.
const GUTTER: f32 = 16.0;

/// The widest your bubble runs, as a share of the column.
const BUBBLE_SHARE: f32 = 0.8;

/// The tallest the composer's text grows before it scrolls.
const COMPOSER_TEXT_HEIGHT: f32 = 200.0;

/// Taller than any conversation draws, but finite, so the layout's sums
/// stay numbers: the column's room to grow, and the scroll offset that
/// takes it to its end once egui clamps it.
const UNBOUNDED: f32 = 1.0e7;

/// What a conversation remembers between frames.
#[derive(Clone, Default)]
struct Held {
    /// The first node it showed, which changes when another conversation
    /// takes its place, such as another chat.
    first: Option<String>,
    /// Scroll to the end next frame, as after a press of the button that
    /// goes back down.
    to_end: bool,
    /// How tall its composer drew last frame.
    composer: f32,
    /// How tall an empty conversation's contents drew last frame.
    empty: f32,
}

/// A conversation: it takes the rest of the page, as a split does, with
/// its column in the middle and its composer at the bottom.
pub(super) fn conversation(ui: &mut Ui, node: &ViewNode, label: &str, kit: &mut Kit) {
    let mut available = ui.available_rect_before_wrap();
    available.min.y -= ui.spacing().item_spacing.y;
    let height = ui
        .clip_rect()
        .bottom()
        .min(available.bottom() + 24.0)
        .max(available.top() + 240.0)
        - available.top();
    // It runs past the page's 24-point margins to its edges, so the
    // scroll bar sits at the page's edge; only the page's own width counts
    // toward its size.
    let rect = Rect::from_min_size(
        available.min - vec2(24.0, 0.0),
        vec2(available.width() + 48.0, height),
    );
    ui.allocate_rect(
        Rect::from_min_size(available.min, vec2(available.width(), height)),
        Sense::hover(),
    );
    let (body, composer) = match node.children.split_last() {
        Some((last, rest)) if matches!(last.kind, ViewKind::Composer { .. }) => (rest, Some(last)),
        _ => (node.children.as_slice(), None),
    };
    let column = (rect.width() - 2.0 * GUTTER).clamp(0.0, COLUMN_WIDTH);
    let left = rect.center().x - column / 2.0;
    let held_id = kit.id(&node.id).with("conversation");
    let mut held: Held = ui.data(|data| data.get_temp(held_id)).unwrap_or_default();
    let quiet = std::mem::replace(&mut kit.quiet, true);
    let builder = UiBuilder::new()
        .max_rect(rect)
        .layout(Layout::top_down(Align::Min))
        .accessibility_role(Role::Log)
        .accessibility_label(label);
    let mut log = ui.new_child(builder);
    let empty = body
        .iter()
        .all(|child| matches!(child.kind, ViewKind::Empty { .. }));
    if empty {
        // An empty conversation and its composer stand together a little
        // above the middle of the page.
        let top = rect.top() + ((rect.height() - held.empty) / 2.0 - 48.0).max(GUTTER);
        let area = Rect::from_min_max(pos2(left, top), pos2(left + column, rect.bottom()));
        let mut column_ui = log.new_child(
            UiBuilder::new()
                .max_rect(area)
                .layout(Layout::top_down(Align::Min)),
        );
        column_ui.spacing_mut().item_spacing.y = 16.0;
        for child in body {
            super::node(&mut column_ui, child, kit);
        }
        if let Some(composer) = composer {
            super::node(&mut column_ui, composer, kit);
        }
        let drawn = column_ui.min_rect().height();
        if (drawn - held.empty).abs() > 0.5 {
            held.empty = drawn;
            ui.ctx().request_repaint();
        }
    } else {
        let composer_top = match composer {
            Some(composer) => {
                let top = rect.bottom() - GUTTER - held.composer;
                let area = Rect::from_min_max(
                    pos2(left, top),
                    pos2(left + column, rect.bottom() - GUTTER),
                );
                let mut composer_ui = log.new_child(
                    UiBuilder::new()
                        .max_rect(area)
                        .layout(Layout::top_down(Align::Min)),
                );
                super::node(&mut composer_ui, composer, kit);
                let drawn = composer_ui.min_rect().height();
                if (drawn - held.composer).abs() > 0.5 {
                    held.composer = drawn;
                    ui.ctx().request_repaint();
                }
                top - 8.0
            }
            None => rect.bottom(),
        };
        let messages = Rect::from_min_max(rect.min, pos2(rect.right(), composer_top));
        messages_area(
            &mut log,
            node,
            body,
            (messages, left, column),
            &mut held,
            kit,
        );
    }
    kit.quiet = quiet;
    held.first = body.first().map(|child| child.id.clone());
    ui.data_mut(|data| data.insert_temp(held_id, held));
}

/// The conversation's messages in `area`, in the column at `left`
/// `column` points wide, scrolling on their own and keeping to the end,
/// with a button back to the end while you're scrolled up.
fn messages_area(
    ui: &mut Ui,
    node: &ViewNode,
    body: &[ViewNode],
    (area, left, column): (Rect, f32, f32),
    held: &mut Held,
    kit: &mut Kit,
) {
    let mut area_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(area)
            .layout(Layout::top_down(Align::Min)),
    );
    let mut scroll = ScrollArea::vertical()
        .id_salt((&node.id, "messages"))
        .auto_shrink(false)
        .stick_to_bottom(true);
    // Another conversation, such as another chat, opens at its end.
    let first = body.first().map(|child| child.id.clone());
    if held.to_end || held.first != first {
        scroll = scroll.vertical_scroll_offset(UNBOUNDED);
        held.to_end = false;
    }
    let output = scroll.show(&mut area_ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add_space(24.0);
        let top = ui.cursor().top();
        // The column has no end of its own: it grows with what it holds.
        let rect = Rect::from_min_size(pos2(left, top), vec2(column, UNBOUNDED));
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.set_width(column);
                ui.spacing_mut().item_spacing.y = 12.0;
                blocks(ui, body, kit);
            },
        );
        ui.add_space(24.0);
    });
    let end = output.content_size.y - output.inner_rect.height();
    if output.state.clamped_offset().y < end - 4.0 {
        let button = Rect::from_center_size(
            pos2(area.center().x, area.bottom() - 24.0),
            vec2(32.0, 32.0),
        );
        let mut button_ui = ui.new_child(UiBuilder::new().max_rect(button));
        let palette = Palette::of(ui.ctx());
        if Btn::icon_only(icons::CARET_DOWN, "Scroll to end")
            .variant(Variant::Outline)
            .fill(palette.background)
            .show(&mut button_ui)
            .clicked()
        {
            held.to_end = true;
            ui.ctx().request_repaint();
        }
    }
}

/// The nodes of a conversation or an open activity, top to bottom, with
/// more room before a message and each run of buttons on one row.
fn blocks(ui: &mut Ui, nodes: &[ViewNode], kit: &mut Kit) {
    let mut index = 0;
    while index < nodes.len() {
        let node = &nodes[index];
        if index > 0 && matches!(node.kind, ViewKind::Message { .. }) {
            ui.add_space(12.0);
        }
        let run = nodes[index..]
            .iter()
            .take_while(|node| matches!(node.kind, ViewKind::Button { .. }))
            .count();
        if run > 1 {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for button in &nodes[index..index + run] {
                    super::node(ui, button, kit);
                }
            });
            index += run;
        } else {
            super::node(ui, node, kit);
            index += 1;
        }
    }
}

/// A quiet line of text, as a conversation shows its text.
pub(super) fn quiet_line(ui: &mut Ui, text: &str) {
    let palette = Palette::of(ui.ctx());
    ui.add(
        Label::new(
            RichText::new(text)
                .size(SUPPORTING_SIZE + 1.0)
                .color(palette.muted_foreground),
        )
        .wrap(),
    );
}

/// A message: yours in a bubble at the end of the line, as you wrote it;
/// another's as Markdown at the start, with no bubble. A quiet line and a
/// warning go under it.
pub(super) fn message(ui: &mut Ui, node: &ViewNode, kit: &mut Kit) {
    let ViewKind::Message {
        author,
        label,
        text,
        detail,
        warning,
    } = &node.kind
    else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new()
        .accessibility_role(Role::Article)
        .accessibility_label(label.as_str());
    ui.scope_builder(builder, |ui| {
        ui.set_width(ui.available_width());
        let align = match author {
            MessageAuthor::You => Align::Max,
            MessageAuthor::Other => Align::Min,
        };
        ui.with_layout(Layout::top_down(align), |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            match author {
                MessageAuthor::You => bubble(ui, text, &palette),
                MessageAuthor::Other if text.is_empty() && detail.is_none() => coming(ui),
                MessageAuthor::Other => {
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        if let Some(url) = markdown::show(ui, text) {
                            kit.link = Some(url);
                        }
                    });
                }
            }
            if let Some(detail) = detail {
                quiet_line(ui, detail);
            }
            if let Some(warning) = warning {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    icons::paint(
                        ui.painter(),
                        icons::WARNING,
                        rect,
                        palette.destructive,
                        false,
                    );
                    ui.add(
                        Label::new(
                            RichText::new(warning)
                                .size(SUPPORTING_SIZE + 1.0)
                                .color(palette.destructive),
                        )
                        .wrap(),
                    );
                });
            }
        });
    });
}

/// Your message in its bubble: the secondary fill, as wide as its text
/// up to most of the column.
fn bubble(ui: &mut Ui, text: &str, palette: &Palette) {
    let room = (ui.available_width() * BUBBLE_SHARE - 32.0).max(48.0);
    let body = font(ui.ctx(), BODY_SIZE, Weight::Regular);
    let wrapped = ui
        .painter()
        .layout(text.to_owned(), body.clone(), palette.foreground, room);
    let text_size = vec2(wrapped.size().x.ceil().min(room), wrapped.size().y.ceil());
    let padding = vec2(16.0, 10.0);
    let (rect, _) = ui.allocate_exact_size(text_size + padding * 2.0, Sense::hover());
    // A one-line message is a pill; a longer one keeps the same corners.
    ui.painter().rect_filled(rect, pill(), palette.secondary);
    let inner = Rect::from_min_size(rect.min + padding, text_size);
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.add(Label::new(RichText::new(text).font(body).color(palette.foreground)).wrap());
        },
    );
}

/// Three dots that say an answer has started and more is coming.
fn coming(ui: &mut Ui) {
    let palette = Palette::of(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 20.0), Sense::hover());
    for index in 0..3 {
        let center = pos2(rect.left() + 6.0 + index as f32 * 10.0, rect.center().y);
        ui.painter()
            .circle_filled(center, 3.0, palette.muted_foreground.gamma_multiply(0.7));
    }
}

/// What a step's state reads as, and the icon and color it shows in.
fn state_look(state: ActivityState, palette: &Palette) -> (&'static str, icons::Icon, Color32) {
    match state {
        ActivityState::Waiting => ("Waiting", icons::CIRCLE, palette.muted_foreground),
        ActivityState::Running => ("Running", icons::SPINNER, palette.muted_foreground),
        ActivityState::Done => ("Done", icons::DONE, palette.muted_foreground),
        ActivityState::Failed => ("Failed", icons::FAILED, palette.destructive),
        ActivityState::Denied => ("Denied", icons::CLOSE, palette.muted_foreground),
        ActivityState::Stopped => ("Stopped", icons::STOP, palette.muted_foreground),
    }
}

/// A step taken along the way: a quiet row with its state's icon, its
/// title and, unless it's done, its state. One with children opens on a
/// click and shows them in a box under the row.
pub(super) fn activity(ui: &mut Ui, node: &ViewNode, kit: &mut Kit) {
    let ViewKind::Activity { title, state } = &node.kind else {
        return;
    };
    let palette = Palette::of(ui.ctx());
    let id = kit.id(&node.id);
    let opens = !node.children.is_empty();
    let mut open: bool = opens && ui.data(|data| data.get_temp(id)).unwrap_or(false);
    let (said, icon, color) = state_look(*state, &palette);
    let height = 28.0;
    let width = ui.available_width();
    let text_font = font(ui.ctx(), SUPPORTING_SIZE + 1.0, Weight::Regular);
    let badge_font = font(ui.ctx(), SUPPORTING_SIZE, Weight::Medium);
    let badge = (*state != ActivityState::Done).then(|| {
        ui.painter()
            .layout_no_wrap(said.to_owned(), badge_font, color)
    });
    let badge_width = badge
        .as_ref()
        .map_or(0.0, |galley| galley.size().x + 16.0 + 8.0);
    let caret_width = if opens { 24.0 } else { 0.0 };
    let title_room = (width - 8.0 - 22.0 - badge_width - caret_width - 8.0).max(24.0);
    let title_galley = egui::WidgetText::from(RichText::new(title).font(text_font)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        title_room,
        egui::TextStyle::Small,
    );
    let row_width =
        (8.0 + 22.0 + title_galley.size().x + badge_width + caret_width + 8.0).min(width);
    let sense = if opens {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(row_width, height), sense);
    let role = if opens { Role::Button } else { Role::Label };
    response.widget_info(|| WidgetInfo::labeled(role, true, title.as_str()));
    let spoken = said.to_owned();
    ui.ctx().accesskit_node_builder(response.id, |builder| {
        builder.set_description(spoken);
        if opens {
            builder.set_expanded(open);
        }
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if opens && response.hovered() {
            painter.rect_filled(rect, radius(0.8), palette.accent);
        }
        if response.has_focus() {
            crate::common::focus_ring(painter, rect, radius(0.8), &palette);
        }
        let mut x = rect.left() + 8.0;
        let square = Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 14.0));
        icons::paint(painter, icon, square, color, false);
        x += 22.0;
        let text_color = if *state == ActivityState::Failed {
            palette.destructive
        } else {
            palette.muted_foreground
        };
        let y = rect.center().y - title_galley.size().y / 2.0;
        let title_width = title_galley.size().x;
        painter.galley(pos2(x, y), title_galley, text_color);
        x += title_width + 8.0;
        if let Some(badge) = badge {
            let pill_rect = Rect::from_min_size(
                pos2(x, rect.center().y - 10.0),
                vec2(badge.size().x + 16.0, 20.0),
            );
            painter.rect_filled(pill_rect, pill(), palette.secondary);
            painter.galley(pill_rect.center() - badge.size() / 2.0, badge, color);
            x = pill_rect.right() + 8.0;
        }
        if opens {
            let caret = if open {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            };
            let square = Rect::from_center_size(pos2(x + 6.0, rect.center().y), vec2(12.0, 12.0));
            icons::paint(painter, caret, square, palette.muted_foreground, false);
        }
    }
    if response.clicked() {
        open = !open;
    }
    if opens {
        ui.data_mut(|data| data.insert_temp(id, open));
    }
    if open {
        Frame::new()
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(radius(1.0))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.0;
                blocks(ui, &node.children, kit);
            });
    }
}

/// What a composer remembers between frames, in egui's memory under its
/// ID.
#[derive(Clone)]
struct Draft {
    /// The value the plugin drew last.
    drawn: String,
    text: String,
}

/// A composer's label, its value, its send button's label and its stop
/// button's label while something is under way.
pub(super) struct ComposerParts<'a> {
    pub(super) label: &'a str,
    pub(super) value: &'a str,
    pub(super) send: &'a str,
    pub(super) stop: Option<&'a str>,
}

/// A box to write a message in: a card with the text on top and a row
/// under it of its choices and buttons, then the send or stop button.
pub(super) fn composer(ui: &mut Ui, node: &ViewNode, parts: &ComposerParts, kit: &mut Kit) {
    let palette = Palette::of(ui.ctx());
    let id = kit.id(&node.id);
    let text_id = id.with("text");
    let mut draft = ui
        .data(|data| data.get_temp::<Draft>(id))
        .filter(|draft| draft.drawn == parts.value)
        .unwrap_or_else(|| Draft {
            drawn: parts.value.to_owned(),
            text: parts.value.to_owned(),
        });
    let focused = ui.memory(|memory| memory.has_focus(text_id));
    let entered =
        focused && ui.input(|input| input.key_pressed(Key::Enter) && input.modifiers.is_none());
    let dark = ui.visuals().dark_mode;
    let fill = if dark {
        crate::common::mix(palette.background, palette.input, 0.3)
    } else {
        palette.background
    };
    let stroke = if focused { palette.ring } else { palette.input };
    let mut send = false;
    let mut stop = false;
    let builder = UiBuilder::new()
        .accessibility_role(Role::Form)
        .accessibility_label(parts.label);
    ui.scope_builder(builder, |ui| {
        Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(radius(2.4))
            .inner_margin(Margin {
                left: 8,
                right: 8,
                top: 6,
                bottom: 8,
            })
            .shadow(Shadow {
                offset: [0, 2],
                blur: 8,
                spread: 0,
                color: Color32::from_black_alpha(if dark { 40 } else { 12 }),
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ScrollArea::vertical()
                    .id_salt((&node.id, "text"))
                    .max_height(COMPOSER_TEXT_HEIGHT)
                    .show(ui, |ui| {
                        let response = ui
                            .add(
                                TextEdit::multiline(&mut draft.text)
                                    .id(text_id)
                                    .hint_text(parts.label)
                                    .frame(Frame::NONE.inner_margin(Margin::symmetric(12, 8)))
                                    .desired_rows(1)
                                    .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                                    .desired_width(f32::INFINITY),
                            )
                            .accessible_name(parts.label);
                        // Clicking the card's empty room writes in the box.
                        if response.clicked() {
                            response.request_focus();
                        }
                    });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for child in &node.children {
                        ui.push_id(&child.id, |ui| match &child.kind {
                            ViewKind::Input { .. } => form::picker(ui, child, kit),
                            ViewKind::Button { label } => tool_button(ui, child, label, kit),
                            _ => {}
                        });
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        match parts.stop {
                            Some(label) => {
                                stop = Btn::icon_only(icons::STOP, label)
                                    .variant(Variant::Default)
                                    .show(ui)
                                    .clicked();
                            }
                            None => {
                                let ready = !draft.text.trim().is_empty();
                                let pressed = Btn::icon_only(icons::SEND, parts.send)
                                    .variant(Variant::Default)
                                    .enabled(ready)
                                    .show(ui)
                                    .clicked();
                                send = ready && (pressed || entered);
                            }
                        }
                    });
                });
            });
    });
    if stop {
        kit.events.push(PluginEvent::Pressed(node.id.clone()));
    }
    if send {
        let mut fields = vec![Field {
            node: node.id.clone(),
            value: InputValue::Multiline(draft.text.clone()),
        }];
        let mut whole = true;
        form::inputs(ui, node, kit, &mut fields, &mut whole);
        kit.events.push(PluginEvent::Submitted(Submission {
            node: node.id.clone(),
            fields,
        }));
        draft.text.clear();
        ui.memory_mut(|memory| memory.request_focus(text_id));
    }
    ui.data_mut(|data| data.insert_temp(id, draft));
}

/// A button in a composer's row, such as one that attaches a file: small,
/// with no fill until you point at it.
fn tool_button(ui: &mut Ui, node: &ViewNode, label: &str, kit: &mut Kit) {
    let pressed = Btn::new(label)
        .variant(Variant::Ghost)
        .size(Size::Sm)
        .show(ui)
        .clicked();
    if pressed {
        kit.events.push(PluginEvent::Pressed(node.id.clone()));
    }
}
