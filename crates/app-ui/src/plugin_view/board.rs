//! Boards: columns side by side, each a header over its cards. You select
//! a card by click or the arrow keys, which move up and down a column and
//! across to the next column with cards, and open it with Enter or a
//! double-click. A card's menu opens by right-click, the context-menu key
//! or Shift-F10. Selection follows the keyboard, and the board is one stop
//! in the Tab order, on its selected card.
//!
//! You drag a card within its column or into another column of the same
//! board. A line shows where it would land, and a drop sends one move
//! event; the board then shows the cards where the plugin draws them.

use app_core::{PluginEvent, PluginMove, Target, ViewKind, ViewNode};
use egui::{
    Align2, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, UiBuilder, WidgetInfo,
    accesskit::{Action, HasPopup, Role},
    pos2, vec2,
};

use super::{
    Kit, drag, menu,
    rows::{ListKey, key_filter, take_keys, text},
};
use crate::theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, radius};

/// A column's width and the space between columns.
const COLUMN_WIDTH: f32 = 256.0;
const COLUMN_GAP: f32 = 8.0;
/// The height of a column's header.
const HEADER_HEIGHT: f32 = 36.0;
/// The space around and between a column's cards.
const PADDING: f32 = 6.0;
/// A card's height with a label alone, and with a detail under it.
const CARD_HEIGHT: f32 = 40.0;
const CARD_WITH_DETAIL_HEIGHT: f32 = 58.0;
/// The least a column's body holds, so an empty column takes a drop.
const MIN_BODY: f32 = 64.0;

struct Card<'a> {
    id: &'a str,
    label: &'a str,
    detail: Option<&'a str>,
    menu: Option<&'a ViewNode>,
}

struct Column<'a> {
    id: &'a str,
    label: &'a str,
    cards: Vec<Card<'a>>,
}

fn columns(board: &ViewNode) -> Vec<Column<'_>> {
    board
        .children
        .iter()
        .filter_map(|column| {
            let ViewKind::BoardColumn { label } = &column.kind else {
                return None;
            };
            let cards = column
                .children
                .iter()
                .filter_map(|card| {
                    let ViewKind::Card { label, detail } = &card.kind else {
                        return None;
                    };
                    Some(Card {
                        id: &card.id,
                        label,
                        detail: detail.as_deref(),
                        menu: card
                            .children
                            .iter()
                            .find(|child| matches!(child.kind, ViewKind::Menu { .. })),
                    })
                })
                .collect();
            Some(Column {
                id: &column.id,
                label,
                cards,
            })
        })
        .collect()
}

/// Where a card sits: its column and its place in the column.
type Place = (usize, usize);

fn find(columns: &[Column], card: &str) -> Option<Place> {
    columns.iter().enumerate().find_map(|(column, cards)| {
        let index = cards.cards.iter().position(|each| each.id == card)?;
        Some((column, index))
    })
}

/// Where the keyboard goes from `from` with `key`: up and down within a
/// column, and left and right to the nearest column with cards, at the
/// same place or its last card.
fn step(columns: &[Column], from: Place, key: ListKey) -> Place {
    let (column, index) = from;
    let last = columns[column].cards.len().saturating_sub(1);
    let across = |other: Option<usize>| {
        other.map_or(from, |other| {
            (other, index.min(columns[other].cards.len() - 1))
        })
    };
    let has_cards = |other: &usize| !columns[*other].cards.is_empty();
    match key {
        ListKey::Up => (column, index.saturating_sub(1)),
        ListKey::Down => (column, (index + 1).min(last)),
        ListKey::Home => (column, 0),
        ListKey::End => (column, last),
        ListKey::Left => across((0..column).rev().find(has_cards)),
        ListKey::Right => across((column + 1..columns.len()).find(has_cards)),
        ListKey::Activate | ListKey::Select | ListKey::Menu => from,
    }
}

/// What a board remembers between frames, in egui's memory under its ID.
#[derive(Clone, Default)]
struct State {
    /// The selected card's node ID.
    selected: Option<String>,
    menu: Option<OpenMenu>,
}

#[derive(Clone)]
struct OpenMenu {
    card: String,
    /// Where a right-click opened it. A menu opened from the keyboard opens
    /// below its card.
    at: Option<Pos2>,
    focus_first: bool,
}

/// The rectangles a board's columns and cards take.
struct Layout {
    /// Each column's whole rectangle and its body below the header.
    columns: Vec<(Rect, Rect)>,
    /// Each column's cards.
    cards: Vec<Vec<Rect>>,
}

fn card_height(card: &Card) -> f32 {
    if card.detail.is_some() {
        CARD_WITH_DETAIL_HEIGHT
    } else {
        CARD_HEIGHT
    }
}

fn layout(columns: &[Column], top_left: Pos2) -> (Layout, Rect) {
    let body = columns
        .iter()
        .map(|column| {
            let cards: f32 = column.cards.iter().map(card_height).sum();
            let gaps = column.cards.len().saturating_sub(1) as f32 * PADDING;
            cards + gaps + PADDING * 2.0
        })
        .fold(MIN_BODY, f32::max);
    let mut layout = Layout {
        columns: Vec::new(),
        cards: Vec::new(),
    };
    for (index, column) in columns.iter().enumerate() {
        let left = top_left.x + index as f32 * (COLUMN_WIDTH + COLUMN_GAP);
        let rect = Rect::from_min_size(
            pos2(left, top_left.y),
            vec2(COLUMN_WIDTH, HEADER_HEIGHT + body),
        );
        let body_rect = Rect::from_min_max(pos2(left, rect.top() + HEADER_HEIGHT), rect.max);
        let mut y = body_rect.top() + PADDING;
        let cards = column
            .cards
            .iter()
            .map(|card| {
                let height = card_height(card);
                let card = Rect::from_min_size(
                    pos2(left + PADDING, y),
                    vec2(COLUMN_WIDTH - PADDING * 2.0, height),
                );
                y += height + PADDING;
                card
            })
            .collect();
        layout.columns.push((rect, body_rect));
        layout.cards.push(cards);
    }
    let width = columns.len() as f32 * (COLUMN_WIDTH + COLUMN_GAP) - COLUMN_GAP;
    let whole = Rect::from_min_size(top_left, vec2(width.max(0.0), HEADER_HEIGHT + body));
    (layout, whole)
}

pub(super) fn board(ui: &mut Ui, node: &ViewNode, label: &str, kit: &mut Kit) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Group)
        .accessibility_label(label);
    ui.scope_builder(builder, |ui| draw(ui, node, kit));
}

fn draw(ui: &mut Ui, board: &ViewNode, kit: &mut Kit) {
    let board_id = kit.id(&board.id);
    let card_id = |card: &str| board_id.with(("card", card));
    let mut state: State = ui.data(|data| data.get_temp(board_id)).unwrap_or_default();
    let columns = columns(board);
    // A redraw that takes the selected card away clears the selection.
    state.selected = state
        .selected
        .take()
        .filter(|card| find(&columns, card).is_some());
    let first = columns
        .iter()
        .enumerate()
        .find(|(_, column)| !column.cards.is_empty())
        .map(|(index, _)| (index, 0));
    let tab_stop = state
        .selected
        .as_deref()
        .and_then(|card| find(&columns, card))
        .or(first);

    // The board has the keyboard when one of its cards has focus, or when a
    // screen reader asks to focus one. An open menu keeps it.
    let all_cards = || {
        columns.iter().enumerate().flat_map(|(column, cards)| {
            cards
                .cards
                .iter()
                .enumerate()
                .map(move |(index, card)| ((column, index), card.id))
        })
    };
    let asked = ui.input(|input| {
        all_cards()
            .find(|(_, card)| input.has_accesskit_action_request(card_id(card), Action::Focus))
            .map(|(place, _)| place)
    });
    let focused = asked.or_else(|| {
        let focused = ui.memory(|memory| memory.focused())?;
        all_cards()
            .find(|(_, card)| card_id(card) == focused)
            .map(|(place, _)| place)
    });
    let mut keyboard = focused;
    let mut moved = asked.is_some();
    let mut activate = None;
    let mut open_menu = None;
    if let Some(mut current) = focused.filter(|_| state.menu.is_none()) {
        for key in take_keys(ui, true) {
            match key {
                ListKey::Activate => activate = Some(current),
                ListKey::Menu => open_menu = Some(current),
                ListKey::Select => {}
                _ => current = step(&columns, current, key),
            }
        }
        moved |= Some(current) != focused;
        keyboard = Some(current);
    }
    let mut select = keyboard.filter(|_| moved);

    let (layout, whole) = layout(&columns, ui.cursor().min);
    ui.allocate_rect(whole, Sense::hover());
    let palette = Palette::of(ui.ctx());
    let dragging = drag::dragging(ui, board_id);
    let selected_place = state
        .selected
        .as_deref()
        .and_then(|card| find(&columns, card));

    let mut clicked = None;
    let mut double_clicked = None;
    let mut right_clicked = None;
    let mut gained = None;
    let mut chosen = None;
    let mut menu_closed = false;
    for (column_index, column) in columns.iter().enumerate() {
        let (rect, body) = layout.columns[column_index];
        paint_column(ui, rect, body, column, &palette);
        let builder = UiBuilder::new()
            .id_salt(("column", column.id))
            .max_rect(rect)
            .accessibility_role(Role::List)
            .accessibility_label(column.label);
        let mut column_ui = ui.new_child(builder);
        for (index, card) in column.cards.iter().enumerate() {
            let place = (column_index, index);
            let id = card_id(card.id);
            let rect = layout.cards[column_index][index];
            let sense = if Some(place) == tab_stop {
                Sense::click_and_drag()
            } else {
                Sense::CLICK | Sense::DRAG
            };
            let response = column_ui.interact(rect, id, sense);
            drag::start(&response, board_id, card.id, card.label);
            let is_selected = Some(place) == selected_place;
            response.widget_info(|| WidgetInfo::labeled(Role::ListItem, true, card.label));
            column_ui.ctx().accesskit_node_builder(id, |node| {
                node.add_action(Action::Focus);
                node.set_selected(is_selected);
                node.set_position_in_set(index + 1);
                node.set_size_of_set(column.cards.len());
                if let Some(detail) = card.detail {
                    node.set_description(detail);
                }
                if card.menu.is_some() {
                    node.set_has_popup(HasPopup::Menu);
                }
            });
            let lifted = dragging
                .as_ref()
                .is_some_and(|dragged| dragged.item == card.id);
            paint_card(
                &column_ui,
                rect,
                &response,
                card,
                is_selected,
                lifted,
                &palette,
            );
            if response.clicked() {
                clicked = Some(place);
            }
            if response.double_clicked() {
                double_clicked = Some(place);
            }
            if response.secondary_clicked() {
                right_clicked = Some((place, ui.input(|input| input.pointer.interact_pos())));
            }
            if response.gained_focus() {
                gained = Some(place);
                response.scroll_to_me(None);
            }
            if Some(place) == keyboard && moved && !response.has_focus() {
                response.request_focus();
                response.scroll_to_me(None);
            }
            if response.has_focus() {
                column_ui.memory_mut(|memory| memory.set_focus_lock_filter(id, key_filter(true)));
            }
            if let (Some(open), Some(menu_node)) = (state.menu.as_mut(), card.menu)
                && open.card == card.id
            {
                match menu::show(
                    &mut column_ui,
                    &response,
                    menu_node,
                    open.at,
                    &mut open.focus_first,
                ) {
                    menu::Outcome::Open => {}
                    menu::Outcome::Closed => menu_closed = true,
                    menu::Outcome::Chose(item) => chosen = Some(item),
                }
                if menu_closed || chosen.is_some() {
                    response.request_focus();
                }
            }
        }
    }
    if menu_closed || chosen.is_some() {
        state.menu = None;
    }
    // A redraw took away the card with the open menu, or the card's menu.
    if let Some(open) = &state.menu
        && find(&columns, &open.card)
            .is_none_or(|(column, index)| columns[column].cards[index].menu.is_none())
    {
        state.menu = None;
    }
    if let Some(item) = chosen {
        kit.events.push(PluginEvent::Chosen(item));
    }
    if let Some(dragged) = dragging {
        drop_on_board(ui, kit, board, &columns, &layout, &dragged);
    }

    // Pointer and focus changes, then what the keys asked for.
    let right_place = right_clicked.map(|(place, _)| place);
    if let Some(place) = clicked.or(gained).or(right_place) {
        select = Some(place);
    }
    for (column, index) in [clicked, right_place].into_iter().flatten() {
        let id = card_id(columns[column].cards[index].id);
        ui.memory_mut(|memory| memory.request_focus(id));
    }
    if let Some((column, index)) = select {
        let card = columns[column].cards[index].id;
        if state.selected.as_deref() != Some(card) {
            kit.events.push(PluginEvent::Selected(Target {
                node: board.id.clone(),
                item: card.to_owned(),
            }));
            state.selected = Some(card.to_owned());
        }
    }
    for (column, index) in [activate, double_clicked].into_iter().flatten() {
        let card = columns[column].cards[index].id;
        if state.selected.as_deref() == Some(card) {
            kit.events.push(PluginEvent::Activated(Target {
                node: board.id.clone(),
                item: card.to_owned(),
            }));
        }
    }
    let menu_at = open_menu.map(|place| (place, None)).or(right_clicked);
    if let Some(((column, index), at)) = menu_at {
        let card = &columns[column].cards[index];
        if card.menu.is_some() {
            state.menu = Some(OpenMenu {
                card: card.id.to_owned(),
                at,
                focus_first: at.is_none(),
            });
        }
    }
    ui.data_mut(|data| data.insert_temp(board_id, state));
}

/// While a card of `board` is dragged, the line where it would land, and
/// on release the move event, unless it would land where it is.
fn drop_on_board(
    ui: &mut Ui,
    kit: &mut Kit,
    board: &ViewNode,
    columns: &[Column],
    layout: &Layout,
    dragged: &drag::Dragged,
) {
    drag::follow_pointer(ui, &dragged.label);
    let Some(from) = find(columns, &dragged.item) else {
        return;
    };
    let Some(pointer) = drag::pointer(ui) else {
        return;
    };
    let Some(to) = layout
        .columns
        .iter()
        .position(|(rect, _)| rect.contains(pointer))
    else {
        return;
    };
    // The cards the dragged card would land among, and how many of them
    // sit above the pointer.
    let others: Vec<Rect> = layout.cards[to]
        .iter()
        .enumerate()
        .filter(|&(index, _)| (to, index) != from)
        .map(|(_, rect)| *rect)
        .collect();
    let position = others
        .iter()
        .take_while(|rect| rect.center().y < pointer.y)
        .count();
    if (to, position) == from {
        return;
    }
    let body = layout.columns[to].1;
    let y = match (
        position.checked_sub(1).map(|above| others[above]),
        others.get(position),
    ) {
        (Some(above), _) => above.bottom() + PADDING / 2.0,
        (None, Some(below)) => below.top() - PADDING / 2.0,
        (None, None) => body.top() + PADDING / 2.0,
    };
    let column = &columns[to];
    let label = format!(
        "{} goes to {}, position {}",
        dragged.label,
        column.label,
        position + 1
    );
    drag::indicator(
        ui,
        pos2(body.left() + PADDING, y),
        body.width() - PADDING * 2.0,
        &label,
    );
    if drag::released(ui) {
        drag::finish(ui);
        kit.events.push(PluginEvent::Moved(PluginMove {
            node: board.id.clone(),
            item: dragged.item.clone(),
            to: column.id.to_owned(),
            position: u32::try_from(position).unwrap_or(u32::MAX),
        }));
    }
}

/// A column's box, its header's label and its count of cards.
fn paint_column(ui: &Ui, rect: Rect, body: Rect, column: &Column, palette: &Palette) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(body, radius(1.0), palette.muted);
    painter.rect_stroke(
        rect,
        radius(1.0),
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );
    painter.line_segment(
        [body.left_top(), body.right_top()],
        Stroke::new(1.0, palette.border),
    );
    let count = text(
        ui,
        &column.cards.len().to_string(),
        COLUMN_WIDTH,
        SUPPORTING_SIZE,
    );
    let count_width = count.size().x;
    let header_mid = rect.top() + HEADER_HEIGHT / 2.0;
    let at = Align2::RIGHT_CENTER.anchor_size(pos2(rect.right() - 12.0, header_mid), count.size());
    painter.galley(at.min, count, palette.muted_foreground);
    let title = text(
        ui,
        column.label,
        COLUMN_WIDTH - 36.0 - count_width,
        SUPPORTING_SIZE,
    );
    let at = Align2::LEFT_CENTER.anchor_size(pos2(rect.left() + 12.0, header_mid), title.size());
    painter.galley(at.min, title, palette.muted_foreground);
}

/// A card's box, focus ring, label and detail. A card being dragged shows
/// faded in its place.
fn paint_card(
    ui: &Ui,
    rect: Rect,
    response: &Response,
    card: &Card,
    selected: bool,
    lifted: bool,
    palette: &Palette,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let fill = if selected || response.hovered() {
        palette.accent
    } else {
        palette.background
    };
    let (fill, foreground) = if lifted {
        (palette.muted, palette.muted_foreground)
    } else {
        (fill, palette.foreground)
    };
    painter.rect(
        rect,
        radius(0.6),
        fill,
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect.shrink(1.0),
            radius(0.6),
            Stroke::new(1.0, palette.ring),
            StrokeKind::Inside,
        );
    }
    let width = rect.width() - 24.0;
    let label = text(ui, card.label, width, BODY_SIZE);
    let label_top = rect.top() + 10.0;
    painter.galley(pos2(rect.left() + 12.0, label_top), label, foreground);
    if let Some(detail) = card.detail {
        let detail = text(ui, detail, width, SUPPORTING_SIZE);
        painter.galley(
            pos2(rect.left() + 12.0, label_top + 22.0),
            detail,
            palette.muted_foreground,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{Card, Column, ListKey, step};

    fn column<'a>(id: &'a str, cards: &[&'a str]) -> Column<'a> {
        Column {
            id,
            label: id,
            cards: cards
                .iter()
                .map(|card| Card {
                    id: card,
                    label: card,
                    detail: None,
                    menu: None,
                })
                .collect(),
        }
    }

    #[test]
    fn left_and_right_skip_columns_without_cards_and_keep_the_place_where_they_can() {
        let columns = [
            column("todo", &["a", "b", "c"]),
            column("empty", &[]),
            column("done", &["d"]),
        ];
        assert_eq!(step(&columns, (0, 2), ListKey::Right), (2, 0));
        assert_eq!(step(&columns, (2, 0), ListKey::Left), (0, 0));
        assert_eq!(step(&columns, (2, 0), ListKey::Right), (2, 0));
        assert_eq!(step(&columns, (0, 2), ListKey::Down), (0, 2));
        assert_eq!(step(&columns, (0, 1), ListKey::Home), (0, 0));
        assert_eq!(step(&columns, (0, 0), ListKey::End), (0, 2));
    }
}
