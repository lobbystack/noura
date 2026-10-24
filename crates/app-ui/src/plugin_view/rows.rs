//! Lists, virtual lists and tables: rows you select by click or the arrow
//! keys, Home and End, and open with Enter or a double-click. A row's menu
//! opens by right-click, the context-menu key or Shift-F10. Selection
//! follows the keyboard, as in a list box, and each list is one stop in
//! the Tab order, on its selected row.
//!
//! Only the rows in view are drawn, plus the row with the keyboard, which
//! keeps its focus while it is scrolled away. A virtual list asks
//! `app-core` for the rows it draws, which fetches them from the plugin.
//!
//! You can drag the items of a list the plugin lets you reorder. A drop
//! sends one move event, and the list shows the order the plugin draws.

use std::ops::Range;

use app_core::{
    Column, ColumnPress, PluginEvent, PluginMove, SortOrder, Target, ViewKind, ViewNode,
};
use egui::{
    Align2, Event, EventFilter, FontId, Id, Key, Modifiers, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, TextWrapMode, Ui, UiBuilder, WidgetInfo, WidgetText,
    accesskit::{Action, HasPopup, Role, SortDirection},
    pos2, vec2,
};

use super::{Kit, drag, menu};
use crate::theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, radius};

/// The height of a row and of a table's header.
pub(super) const ROW_HEIGHT: f32 = 28.0;

/// Keys a list takes while one of its rows has focus. A board takes the
/// left and right arrows as well.
pub(super) const fn key_filter(horizontal: bool) -> EventFilter {
    EventFilter {
        tab: false,
        horizontal_arrows: horizontal,
        vertical_arrows: true,
        escape: false,
    }
}

#[derive(Clone, Copy)]
pub(super) enum ListKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Activate,
    Select,
    Menu,
}

fn list_key(key: Key, modifiers: Modifiers, horizontal: bool) -> Option<ListKey> {
    if modifiers.matches_exact(Modifiers::SHIFT) {
        return (key == Key::F10).then_some(ListKey::Menu);
    }
    if modifiers.any() {
        return None;
    }
    Some(match key {
        Key::ArrowUp => ListKey::Up,
        Key::ArrowDown => ListKey::Down,
        Key::ArrowLeft if horizontal => ListKey::Left,
        Key::ArrowRight if horizontal => ListKey::Right,
        Key::Home => ListKey::Home,
        Key::End => ListKey::End,
        Key::Enter => ListKey::Activate,
        Key::Space => ListKey::Select,
        Key::Menu => ListKey::Menu,
        _ => return None,
    })
}

/// Take the list's keys out of this frame's input, so neither egui's focus
/// moves nor a click on the focused row sees them. `horizontal` takes the
/// left and right arrows too.
pub(super) fn take_keys(ui: &Ui, horizontal: bool) -> Vec<ListKey> {
    ui.input_mut(|input| {
        let mut taken = Vec::new();
        input.events.retain(|event| {
            let key = match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => list_key(*key, *modifiers, horizontal),
                _ => None,
            };
            match key {
                Some(key) => {
                    taken.push(key);
                    false
                }
                None => true,
            }
        });
        taken
    })
}

/// One row as a list or a table draws it.
struct Row<'a> {
    id: String,
    label: String,
    detail: Option<String>,
    cells: Vec<String>,
    menu: Option<&'a ViewNode>,
}

impl<'a> Row<'a> {
    fn of(node: &'a ViewNode) -> Self {
        let (label, detail, cells) = match &node.kind {
            ViewKind::Item { label, detail } => (label.clone(), detail.clone(), Vec::new()),
            ViewKind::Row { cells } => (row_label(cells), None, cells.clone()),
            _ => (String::new(), None, Vec::new()),
        };
        Self {
            id: node.id.clone(),
            label,
            detail,
            cells,
            menu: node
                .children
                .iter()
                .find(|child| matches!(child.kind, ViewKind::Menu { .. })),
        }
    }
}

/// Where a list's rows come from.
enum Source<'a> {
    /// A list's items or a table's rows, all in the view.
    Nodes(&'a [ViewNode]),
    /// A virtual list's rows, fetched as they're drawn.
    Fetched { count: usize },
}

impl<'a> Source<'a> {
    fn count(&self) -> usize {
        match self {
            Self::Nodes(nodes) => nodes.len(),
            Self::Fetched { count } => *count,
        }
    }

    /// The rows at `range`, each `None` while the plugin hasn't returned it.
    fn rows(
        &self,
        kit: &mut Kit,
        list: &str,
        range: Range<usize>,
    ) -> Result<Vec<Option<Row<'a>>>, String> {
        match self {
            Self::Nodes(nodes) => Ok(nodes[range]
                .iter()
                .map(|node| Some(Row::of(node)))
                .collect()),
            Self::Fetched { .. } => {
                let range = to_u32(range.start)..to_u32(range.end);
                let rows = kit.rows(list, range)?;
                Ok(rows
                    .into_iter()
                    .map(|row| {
                        row.map(|row| Row {
                            id: row.id,
                            label: row.item.label,
                            detail: row.item.detail,
                            cells: Vec::new(),
                            menu: None,
                        })
                    })
                    .collect())
            }
        }
    }

    fn row(&self, kit: &mut Kit, list: &str, index: usize) -> Option<Row<'a>> {
        self.rows(kit, list, index..index + 1)
            .ok()?
            .into_iter()
            .next()
            .flatten()
    }
}

/// The name screen readers give a table row: its first cell with text,
/// which `plugin-host` makes sure it has.
fn row_label(cells: &[String]) -> String {
    cells
        .iter()
        .find(|cell| !cell.trim().is_empty())
        .cloned()
        .unwrap_or_default()
}

/// The rows of a list of `count` rows laid out in `area` that meet `clip`.
/// Empty when `clip` holds none of them, even upside down.
fn visible_rows(clip: Rect, area: Rect, count: usize) -> Range<usize> {
    let first = ((clip.top() - area.top()) / ROW_HEIGHT).floor().max(0.0) as usize;
    let last = ((clip.bottom() - area.top()) / ROW_HEIGHT).ceil().max(0.0) as usize;
    let first = first.min(count);
    first..last.clamp(first, count)
}

fn to_u32(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// What a list remembers between frames, in egui's memory under its ID.
#[derive(Clone, Default)]
struct State {
    /// The selected row's ID, and its index when it was selected.
    selected: Option<(String, usize)>,
    /// The rows drawn last frame, to find the one with keyboard focus.
    drawn: Vec<(Id, usize)>,
    menu: Option<OpenMenu>,
}

#[derive(Clone)]
struct OpenMenu {
    row: String,
    /// Where a right-click opened it. A menu opened from the keyboard opens
    /// below its row.
    at: Option<Pos2>,
    focus_first: bool,
}

pub(super) fn list(ui: &mut Ui, node: &ViewNode, label: &str, reorder: bool, kit: &mut Kit) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::ListBox)
        .accessibility_label(label);
    ui.scope_builder(builder, |ui| {
        let kind = if reorder { Kind::Reorder } else { Kind::List };
        rows(ui, kit, node, &Source::Nodes(&node.children), kind);
    });
}

pub(super) fn virtual_list(ui: &mut Ui, node: &ViewNode, label: &str, count: u32, kit: &mut Kit) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::ListBox)
        .accessibility_label(label);
    ui.scope_builder(builder, |ui| {
        let source = Source::Fetched {
            count: count as usize,
        };
        rows(ui, kit, node, &source, Kind::List);
    });
}

pub(super) fn table(ui: &mut Ui, node: &ViewNode, label: &str, columns: &[Column], kit: &mut Kit) {
    let builder = UiBuilder::new()
        .accessibility_role(Role::Table)
        .accessibility_label(label);
    ui.scope_builder(builder, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        header(ui, node, columns, kit);
        rows(
            ui,
            kit,
            node,
            &Source::Nodes(&node.children),
            Kind::Table(columns),
        );
    });
}

/// A table's header row: a column header per column, which sends a press
/// so the plugin can sort.
fn header(ui: &mut Ui, table: &ViewNode, columns: &[Column], kit: &mut Kit) {
    let palette = Palette::of(ui.ctx());
    let builder = UiBuilder::new().accessibility_role(Role::Row);
    ui.scope_builder(builder, |ui| {
        let (_, area) = ui.allocate_space(vec2(ui.available_width(), ROW_HEIGHT));
        ui.painter().line_segment(
            [area.left_bottom(), area.right_bottom()],
            Stroke::new(1.0, palette.border),
        );
        let width = area.width() / columns.len().max(1) as f32;
        for (index, column) in columns.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(area.left() + index as f32 * width, area.top()),
                vec2(width, ROW_HEIGHT),
            );
            let id = kit.id(&format!("{}\u{0}column\u{0}{}", table.id, column.id));
            let response = ui.interact(rect, id, Sense::click());
            response.widget_info(|| {
                WidgetInfo::labeled(Role::ColumnHeader, true, column.title.as_str())
            });
            let direction = column.sorted.map(|order| match order {
                SortOrder::Ascending => SortDirection::Ascending,
                SortOrder::Descending => SortDirection::Descending,
            });
            ui.ctx().accesskit_node_builder(id, |node| {
                if let Some(direction) = direction {
                    node.set_sort_direction(direction);
                }
            });
            if ui.is_rect_visible(rect) {
                let inner = rect.shrink2(vec2(2.0, 2.0));
                if response.hovered() {
                    ui.painter().rect_filled(inner, radius(0.6), palette.accent);
                }
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        inner,
                        radius(0.6),
                        Stroke::new(1.0, palette.ring),
                        StrokeKind::Inside,
                    );
                }
                let galley = text(ui, &column.title, width - 28.0, SUPPORTING_SIZE);
                let at = Align2::LEFT_CENTER
                    .anchor_size(pos2(rect.left() + 8.0, rect.center().y), galley.size());
                let arrow_at = pos2(at.right() + 8.0, rect.center().y);
                ui.painter()
                    .galley(at.min, galley, palette.muted_foreground);
                if let Some(order) = column.sorted {
                    ui.painter().add(arrow(arrow_at, order, &palette));
                }
            }
            if response.clicked() {
                kit.events.push(PluginEvent::Sorted(ColumnPress {
                    node: table.id.clone(),
                    column: column.id.clone(),
                }));
            }
        }
    });
}

/// A small triangle: up for ascending, down for descending.
fn arrow(center: Pos2, order: SortOrder, palette: &Palette) -> Shape {
    let size = 3.5;
    let points = match order {
        SortOrder::Ascending => vec![
            center + vec2(-size, size * 0.5),
            center + vec2(size, size * 0.5),
            center + vec2(0.0, -size * 0.75),
        ],
        SortOrder::Descending => vec![
            center + vec2(-size, -size * 0.5),
            center + vec2(size, -size * 0.5),
            center + vec2(0.0, size * 0.75),
        ],
    };
    Shape::convex_polygon(points, palette.muted_foreground, Stroke::NONE)
}

pub(super) fn text(ui: &Ui, text: &str, width: f32, size: f32) -> std::sync::Arc<egui::Galley> {
    WidgetText::from(text).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        width.max(0.0),
        FontId::proportional(size),
    )
}

/// What a list of rows is.
#[derive(Clone, Copy)]
enum Kind<'a> {
    List,
    /// A list whose items you can drag to new positions.
    Reorder,
    /// A table, whose rows draw a cell per column.
    Table(&'a [Column]),
}

/// The rows of the list or table `list`, drawn at the cursor.
fn rows(ui: &mut Ui, kit: &mut Kit, list: &ViewNode, source: &Source, kind: Kind) {
    let columns = match kind {
        Kind::Table(columns) => Some(columns),
        Kind::List | Kind::Reorder => None,
    };
    let reorder = matches!(kind, Kind::Reorder);
    let list_id = kit.id(&list.id);
    let mut state: State = ui.data(|data| data.get_temp(list_id)).unwrap_or_default();
    let count = source.count();
    let role = if columns.is_some() {
        Role::Row
    } else {
        Role::ListBoxOption
    };
    // A list of nodes finds its selected row by ID, wherever a redraw put
    // it. A virtual list looks for it among the rows it fetches below, and
    // meanwhile keeps the index it had.
    if let Source::Nodes(nodes) = source {
        state.selected = state.selected.take().and_then(|(id, _)| {
            let index = nodes.iter().position(|node| node.id == id)?;
            Some((id, index))
        });
    }
    state.selected = state.selected.filter(|(_, index)| *index < count);
    let row_id = |id: &str| list_id.with(("row", id));

    // The list has the keyboard when one of last frame's rows has focus, or
    // when a screen reader asks to focus one. An open menu keeps it.
    let asked = ui.input(|input| {
        state
            .drawn
            .iter()
            .find(|(id, _)| input.has_accesskit_action_request(*id, Action::Focus))
            .map(|(_, index)| *index)
    });
    // A redraw with fewer rows leaves the keyboard on the last one.
    let focused = asked
        .or_else(|| {
            let focused = ui.memory(|memory| memory.focused())?;
            state
                .drawn
                .iter()
                .find(|(id, _)| *id == focused)
                .map(|(_, index)| *index)
        })
        .and_then(|index| count.checked_sub(1).map(|last| index.min(last)));
    let mut keyboard = focused;
    let mut moved = asked.is_some();
    let mut activate = None;
    let mut open_menu = None;
    if let Some(mut current) = focused.filter(|_| state.menu.is_none() && count > 0) {
        for key in take_keys(ui, false) {
            match key {
                ListKey::Left | ListKey::Right => {}
                ListKey::Up => current = current.saturating_sub(1),
                ListKey::Down => current = (current + 1).min(count - 1),
                ListKey::Home => current = 0,
                ListKey::End => current = count - 1,
                ListKey::Select => {}
                ListKey::Activate => activate = Some(current),
                ListKey::Menu => open_menu = Some(current),
            }
        }
        moved |= Some(current) != focused;
        keyboard = Some(current);
    }
    let mut select = keyboard.filter(|_| moved);

    let (_, area) = ui.allocate_space(vec2(ui.available_width(), count as f32 * ROW_HEIGHT));
    let visible = visible_rows(ui.clip_rect(), area, count);
    // The selected row stays the list's stop in the Tab order, and the row
    // with the keyboard keeps its focus, even scrolled out of view.
    let tab_stop_of = |state: &State| {
        state
            .selected
            .as_ref()
            .map(|(_, index)| *index)
            .or((count > 0).then_some(0))
    };
    let tab_stop = tab_stop_of(&state);
    let mut indices: Vec<usize> = visible.clone().collect();
    for extra in [keyboard, tab_stop].into_iter().flatten() {
        if !indices.contains(&extra) {
            indices.push(extra);
        }
    }
    let mut fetched = match source.rows(kit, &list.id, visible.clone()) {
        Ok(rows) => rows,
        Err(reason) => {
            unavailable(ui, area, &reason);
            return;
        }
    };
    fetched.resize_with(visible.len(), || None);
    let mut rows_by_index: Vec<(usize, Row)> = Vec::with_capacity(indices.len());
    for &index in &indices {
        let row = if visible.contains(&index) {
            fetched[index - visible.start].take()
        } else {
            source.row(kit, &list.id, index)
        };
        if let Some(row) = row {
            rows_by_index.push((index, row));
        }
    }
    // A redraw may have moved a virtual list's selected row, such as when
    // rows came in above it. Where it is among the rows fetched, the
    // selection follows it. Elsewhere the selection stays at its index,
    // and the row there shows it, so the highlight and the Tab stop are
    // always the same row.
    if let Some((id, index)) = &mut state.selected
        && let Some((at, _)) = rows_by_index.iter().find(|(_, row)| row.id == *id)
    {
        *index = *at;
    }
    let tab_stop = tab_stop_of(&state);
    let selected_index = state.selected.as_ref().map(|(_, index)| *index);

    let palette = Palette::of(ui.ctx());
    let selected_id = state.selected.as_ref().map(|(id, _)| id.clone());
    let mut drawn = Vec::with_capacity(indices.len());
    let mut clicked = None;
    let mut double_clicked = None;
    let mut right_clicked = None;
    let mut gained = None;
    let mut chosen = None;
    let mut menu_closed = false;
    for (index, row) in &rows_by_index {
        let index = *index;
        let id = row_id(&row.id);
        let rect = Rect::from_min_size(
            pos2(area.left(), area.top() + index as f32 * ROW_HEIGHT),
            vec2(area.width(), ROW_HEIGHT),
        );
        let is_selected = Some(index) == selected_index;
        let mut sense = if Some(index) == tab_stop {
            Sense::click()
        } else {
            Sense::CLICK
        };
        if reorder {
            sense |= Sense::DRAG;
        }
        let response = ui.interact(rect, id, sense);
        if reorder {
            drag::start(&response, list_id, &row.id, &row.label);
        }
        response.widget_info(|| WidgetInfo::labeled(role, true, row.label.as_str()));
        let detail = row.detail.clone();
        let has_menu = row.menu.is_some();
        ui.ctx().accesskit_node_builder(id, |node| {
            node.add_action(Action::Focus);
            node.set_selected(is_selected);
            node.set_position_in_set(index + 1);
            node.set_size_of_set(count);
            if let Some(detail) = detail {
                node.set_description(detail);
            }
            if has_menu {
                node.set_has_popup(HasPopup::Menu);
            }
        });
        if let Some(columns) = columns {
            cells(ui, rect, id, row, columns, &palette);
        }
        paint_row(
            ui,
            rect,
            &response,
            row,
            is_selected,
            columns.is_some(),
            &palette,
        );
        if response.clicked() {
            clicked = Some(index);
        }
        if response.double_clicked() {
            double_clicked = Some(index);
        }
        if response.secondary_clicked() {
            right_clicked = Some((index, ui.input(|input| input.pointer.interact_pos())));
        }
        if response.gained_focus() {
            gained = Some(index);
            response.scroll_to_me(None);
        }
        if Some(index) == keyboard && moved && !response.has_focus() {
            response.request_focus();
            response.scroll_to_me(None);
        }
        if response.has_focus() {
            ui.memory_mut(|memory| memory.set_focus_lock_filter(id, key_filter(false)));
        }
        if let (Some(open), Some(menu_node)) = (state.menu.as_mut(), row.menu)
            && open.row == row.id
        {
            match menu::show(ui, &response, menu_node, open.at, &mut open.focus_first) {
                menu::Outcome::Open => {}
                menu::Outcome::Closed => menu_closed = true,
                menu::Outcome::Chose(item) => chosen = Some(item),
            }
            if menu_closed || chosen.is_some() {
                response.request_focus();
            }
        }
        drawn.push((id, index));
    }
    state.drawn = drawn;
    if menu_closed || chosen.is_some() {
        state.menu = None;
    }
    // A redraw took away the row with the open menu, or the row's menu.
    // The menu closes and the keyboard goes back to the list's stop in the
    // Tab order, which it selects, as a row does when it takes the
    // keyboard.
    if let Some(open) = &state.menu
        && !rows_by_index
            .iter()
            .any(|(_, row)| row.id == open.row && row.menu.is_some())
    {
        state.menu = None;
        if let Some((index, row)) = rows_by_index.iter().find(|(at, _)| Some(*at) == tab_stop) {
            ui.memory_mut(|memory| memory.request_focus(row_id(&row.id)));
            select = select.or(Some(*index));
        }
    }
    if let Some(item) = chosen {
        kit.events.push(PluginEvent::Chosen(item));
    }
    if reorder && let Source::Nodes(nodes) = source {
        drop_in_list(ui, kit, list, nodes, area);
    }

    // Pointer and focus changes, then what the keys asked for.
    if let Some(index) = clicked.or(gained).or(right_clicked.map(|(index, _)| index)) {
        select = Some(index);
    }
    for index in [clicked, right_clicked.map(|(index, _)| index)]
        .into_iter()
        .flatten()
    {
        if let Some((_, row)) = rows_by_index.iter().find(|(at, _)| *at == index) {
            ui.memory_mut(|memory| memory.request_focus(row_id(&row.id)));
        }
    }
    if let Some(index) = select {
        let id = rows_by_index
            .iter()
            .find(|(at, _)| *at == index)
            .map(|(_, row)| row.id.clone())
            .or_else(|| source.row(kit, &list.id, index).map(|row| row.id));
        if let Some(id) = id
            && selected_id.as_deref() != Some(id.as_str())
        {
            kit.events.push(PluginEvent::Selected(Target {
                node: list.id.clone(),
                item: id.clone(),
            }));
            state.selected = Some((id, index));
        }
    }
    for index in [activate, double_clicked].into_iter().flatten() {
        if let Some(id) = state
            .selected
            .as_ref()
            .filter(|(_, at)| *at == index)
            .map(|(id, _)| id.clone())
        {
            kit.events.push(PluginEvent::Activated(Target {
                node: list.id.clone(),
                item: id,
            }));
        }
    }
    let menu_at = open_menu.map(|index| (index, None)).or(right_clicked);
    if let Some((index, at)) = menu_at
        && let Some((_, row)) = rows_by_index.iter().find(|(i, _)| *i == index)
        && row.menu.is_some()
    {
        state.menu = Some(OpenMenu {
            row: row.id.clone(),
            at,
            focus_first: at.is_none(),
        });
    }
    ui.data_mut(|data| data.insert_temp(list_id, state));
}

/// While an item of the reorderable list `list` is dragged, the line where
/// it would land, and on release the move event, unless it would land
/// where it is.
fn drop_in_list(ui: &mut Ui, kit: &mut Kit, list: &ViewNode, nodes: &[ViewNode], area: Rect) {
    let Some(dragged) = drag::dragging(ui, kit.id(&list.id)) else {
        return;
    };
    drag::follow_pointer(ui, &dragged.label);
    let Some(from) = nodes.iter().position(|node| node.id == dragged.item) else {
        return;
    };
    let Some(pointer) = drag::pointer(ui)
        .filter(|pointer| area.expand2(vec2(0.0, ROW_HEIGHT / 2.0)).contains(*pointer))
    else {
        return;
    };
    // The gap between rows nearest the pointer, then where the item lands
    // once it has left its own place.
    let gap = ((pointer.y - area.top()) / ROW_HEIGHT)
        .round()
        .clamp(0.0, nodes.len() as f32) as usize;
    let position = if gap > from { gap - 1 } else { gap };
    if position == from {
        return;
    }
    let line = pos2(area.left(), area.top() + gap as f32 * ROW_HEIGHT);
    let label = format!("{} goes to position {}", dragged.label, position + 1);
    drag::indicator(ui, line, area.width(), &label);
    if drag::released(ui) {
        drag::finish(ui);
        kit.events.push(PluginEvent::Moved(PluginMove {
            node: list.id.clone(),
            item: dragged.item.clone(),
            to: list.id.clone(),
            position: to_u32(position),
        }));
    }
}

/// Why a virtual list's rows can't be shown, where they would be.
fn unavailable(ui: &mut Ui, area: Rect, reason: &str) {
    let palette = Palette::of(ui.ctx());
    let rect = Rect::from_min_size(area.min, vec2(area.width(), ROW_HEIGHT * 2.0));
    let builder = UiBuilder::new()
        .max_rect(rect)
        .accessibility_role(Role::Alert);
    ui.scope_builder(builder, |ui| {
        ui.label(egui::RichText::new(reason).color(palette.destructive));
    });
}

/// A table row's cells, each a cell in the accessibility tree under the
/// row `row_id`.
fn cells(ui: &mut Ui, rect: Rect, row_id: Id, row: &Row, columns: &[Column], palette: &Palette) {
    let width = rect.width() / columns.len().max(1) as f32;
    for (index, cell) in row.cells.iter().enumerate() {
        let cell_rect = Rect::from_min_size(
            pos2(rect.left() + index as f32 * width, rect.top()),
            vec2(width, rect.height()),
        );
        let builder = UiBuilder::new()
            .id_salt(("cell", index))
            .max_rect(cell_rect)
            .accessibility_parent(row_id)
            .accessibility_role(Role::Cell)
            .accessibility_label(cell.as_str());
        let cell_ui = ui.new_child(builder);
        if cell_ui.is_rect_visible(cell_rect) {
            let galley = text(&cell_ui, cell, width - 16.0, BODY_SIZE);
            let at = Align2::LEFT_CENTER.anchor_size(
                pos2(cell_rect.left() + 8.0, cell_rect.center().y),
                galley.size(),
            );
            cell_ui.painter().galley(at.min, galley, palette.foreground);
        }
    }
}

/// A row's background, focus ring and, for a list, its label and detail.
fn paint_row(
    ui: &Ui,
    rect: Rect,
    response: &Response,
    row: &Row,
    selected: bool,
    table: bool,
    palette: &Palette,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let inner = rect.shrink2(vec2(2.0, 1.0));
    if selected || response.hovered() {
        painter.rect_filled(inner, radius(0.6), palette.accent);
    }
    if response.has_focus() {
        painter.rect_stroke(
            inner,
            radius(0.6),
            Stroke::new(1.0, palette.ring),
            StrokeKind::Inside,
        );
    }
    if table {
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, palette.border),
        );
        return;
    }
    let detail = row
        .detail
        .as_deref()
        .map(|detail| text(ui, detail, inner.width() * 0.4, SUPPORTING_SIZE));
    let detail_width = detail.as_ref().map_or(0.0, |galley| galley.size().x + 16.0);
    let label = text(
        ui,
        &row.label,
        inner.width() - 16.0 - detail_width,
        BODY_SIZE,
    );
    let at =
        Align2::LEFT_CENTER.anchor_size(pos2(inner.left() + 8.0, rect.center().y), label.size());
    painter.galley(at.min, label, palette.foreground);
    if let Some(detail) = detail {
        let at = Align2::RIGHT_CENTER
            .anchor_size(pos2(inner.right() - 8.0, rect.center().y), detail.size());
        painter.galley(at.min, detail, palette.muted_foreground);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Rect, pos2};

    use super::{ROW_HEIGHT, row_label, visible_rows};

    fn band(top: f32, bottom: f32) -> Rect {
        Rect::from_min_max(pos2(0.0, top), pos2(400.0, bottom))
    }

    #[test]
    fn the_visible_rows_are_those_that_meet_the_clip_rect() {
        let area = band(100.0, 100.0 + 50.0 * ROW_HEIGHT);
        assert_eq!(visible_rows(band(0.0, 200.0), area, 50), 0..4);
        assert_eq!(
            visible_rows(band(100.0 + 10.0 * ROW_HEIGHT, 2000.0), area, 50),
            10..50
        );
        // A clip below the list holds none of its rows.
        assert_eq!(visible_rows(band(5000.0, 6000.0), area, 50), 50..50);
    }

    #[test]
    fn an_upside_down_clip_rect_holds_no_rows() {
        let area = band(0.0, 50.0 * ROW_HEIGHT);
        let upside_down = Rect::from_min_max(pos2(0.0, 600.0), pos2(400.0, 100.0));
        assert!(visible_rows(upside_down, area, 50).is_empty());
        assert!(visible_rows(Rect::NOTHING, area, 50).is_empty());
    }

    #[test]
    fn a_table_row_is_named_by_its_first_cell_with_text() {
        let cells = |cells: &[&str]| {
            cells
                .iter()
                .map(|cell| (*cell).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(row_label(&cells(&["Basil", "Herb spiral"])), "Basil");
        assert_eq!(row_label(&cells(&["", " ", "Pots"])), "Pots");
    }
}
