//! Calendar grids: a month of six weeks from Sunday, a week from Sunday to
//! Saturday, or a day, laid out as `app-core` places the plugin's entries,
//! in this computer's time zone and locale. A week and a day have a row of
//! all-day entries above a row per hour, where overlapping entries sit
//! side by side.
//!
//! AccessKit sees a grid: a row of column headers, then rows of cells, one
//! cell per day, or per day and hour. Each entry sits in the cell of its
//! day, and of its start's hour, labeled with its title and time. Today's
//! cell in a month, and today's header in a week or a day, carry the
//! current-date state.
//!
//! You select an entry by click or the keyboard and open it with Enter or
//! a double-click. The arrow keys walk the entries day by day, and Home
//! and End go to the first and last. An entry shows once on each day it
//! covers, and the grid is one stop in the Tab order, on the selected
//! entry where the keyboard last left it.

use app_core::{
    CalendarMode, GridDay, GridEntry, PluginEvent, Span, Target, ViewKind, ViewNode, grid_days,
    lay_out,
};
use egui::{
    Align2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, WidgetInfo,
    accesskit::{Action, AriaCurrent, Role},
    pos2, vec2,
};
use jiff::{Timestamp, civil::Date, tz::TimeZone};

use super::{
    Kit,
    rows::{ListKey, key_filter, take_keys, text},
};
use crate::{
    dates::DateNames,
    theme::{Palette, SUPPORTING_SIZE, TITLE_SIZE, radius},
};

/// The height of an hour in a week or a day, and the hour labels' width.
const HOUR_HEIGHT: f32 = 56.0;
const GUTTER: f32 = 56.0;
/// The height of a week's or a day's header, and of a month's.
const HEADER_HEIGHT: f32 = 40.0;
const MONTH_HEADER_HEIGHT: f32 = 28.0;
/// The least a month's week, or a week's all-day row, is tall.
const MIN_WEEK_HEIGHT: f32 = 96.0;
const MIN_ALL_DAY_HEIGHT: f32 = 48.0;
/// The height of an entry in a month or in the all-day row, and the space
/// between entries.
const ENTRY_HEIGHT: f32 = 22.0;
const ENTRY_GAP: f32 = 2.0;
/// The least a timed entry is tall in a week or a day.
const MIN_TIMED_HEIGHT: f32 = 24.0;
/// The space a day number takes at the top of a month's day.
const DAY_NUMBER_HEIGHT: f32 = 30.0;
/// The narrowest a week's day may be.
const MIN_WEEK_DAY_WIDTH: f32 = 120.0;

/// An entry on one of the grid's days.
struct Shown {
    /// The entry's index among the calendar's entries.
    entry: usize,
    /// The day's index in the grid.
    day: usize,
    /// For a timed entry, its segment's index on the day.
    segment: Option<usize>,
    /// What screen readers say: its title and time.
    label: String,
}

/// A cell of the grid: a header, or a day, or a day's hour, with the
/// entries in it.
struct Cell {
    rect: Rect,
    role: Role,
    label: String,
    /// Whether the cell is today's day, which the grid marks.
    today: bool,
    /// The entries the cell holds, by index into the shown entries, and
    /// where each one draws.
    entries: Vec<(usize, Rect)>,
}

/// A row of the grid.
struct Row {
    rect: Rect,
    cells: Vec<Cell>,
}

/// What a calendar grid remembers between frames, in egui's memory under
/// its ID: the selected entry's node ID and the day the keyboard left it
/// on.
#[derive(Clone, Default)]
struct State {
    selected: Option<(String, Date)>,
}

/// The days, the entries and the time zone of one grid.
struct Grid<'a> {
    names: &'a DateNames,
    time_zone: TimeZone,
    today: Date,
    focus: Date,
    days: Vec<GridDay>,
    entries: Vec<GridEntry<'a>>,
    ids: Vec<&'a str>,
    shown: Vec<Shown>,
}

impl Grid<'_> {
    fn local(&self, at: Timestamp) -> jiff::civil::Time {
        at.to_zoned(self.time_zone.clone()).time()
    }
}

pub(super) fn calendar(
    ui: &mut Ui,
    node: &ViewNode,
    label: &str,
    mode: CalendarMode,
    date: &str,
    kit: &mut Kit,
) {
    // `plugin-host` makes sure the date is one.
    let Ok(focus) = date.parse::<Date>() else {
        return;
    };
    let names = DateNames::of(kit.app.locale().as_deref());
    let time_zone = kit.app.time_zone();
    let mut ids = Vec::new();
    let mut entries = Vec::new();
    for child in &node.children {
        if let ViewKind::Entry { title, start, end } = &child.kind
            && let Some(span) = Span::parse(start, end.as_deref())
        {
            ids.push(child.id.as_str());
            entries.push(GridEntry { title, span });
        }
    }
    let dates = grid_days(mode, focus);
    let days = lay_out(&entries, &dates, &time_zone);
    let mut grid = Grid {
        names: &names,
        time_zone,
        today: kit.app.today(),
        focus,
        days,
        entries,
        ids,
        shown: Vec::new(),
    };
    grid.shown = shown(&grid);

    let heading = match mode {
        CalendarMode::Day => names.full_date(focus),
        CalendarMode::Month | CalendarMode::Week => names.month(focus),
    };
    let builder = UiBuilder::new()
        .accessibility_role(Role::Heading)
        .accessibility_label(heading.as_str());
    ui.scope_builder(builder, |ui| {
        ui.label(RichText::new(&heading).size(TITLE_SIZE).strong());
    });
    let builder = UiBuilder::new()
        .accessibility_role(Role::Grid)
        .accessibility_label(label);
    ui.scope_builder(builder, |ui| draw(ui, node, mode, &grid, kit));
}

/// Every entry on every day, day by day, all-day entries first: the order
/// the arrow keys walk.
fn shown(grid: &Grid) -> Vec<Shown> {
    let mut shown = Vec::new();
    for (day_index, day) in grid.days.iter().enumerate() {
        for &entry in &day.all_day {
            shown.push(Shown {
                entry,
                day: day_index,
                segment: None,
                label: format!("{}, all day", grid.entries[entry].title),
            });
        }
        for (index, segment) in day.timed.iter().enumerate() {
            let times = format!(
                "{}–{}",
                grid.names.time(grid.local(segment.start)),
                grid.names.time(grid.local(segment.end))
            );
            shown.push(Shown {
                entry: segment.entry,
                day: day_index,
                segment: Some(index),
                label: format!("{}, {times}", grid.entries[segment.entry].title),
            });
        }
    }
    shown
}

fn draw(ui: &mut Ui, node: &ViewNode, mode: CalendarMode, grid: &Grid, kit: &mut Kit) {
    let grid_id = kit.id(&node.id);
    let entry_id = |shown: &Shown| {
        grid_id.with((
            "entry",
            grid.ids[shown.entry],
            grid.days[shown.day].date.to_string(),
        ))
    };
    let mut state: State = ui.data(|data| data.get_temp(grid_id)).unwrap_or_default();
    state.selected = state
        .selected
        .take()
        .filter(|(id, _)| grid.ids.contains(&id.as_str()));
    let tab_stop = state
        .selected
        .as_ref()
        .and_then(|(id, day)| {
            let of_entry = |shown: &&Shown| grid.ids[shown.entry] == id;
            grid.shown
                .iter()
                .position(|shown| of_entry(&shown) && grid.days[shown.day].date == *day)
                .or_else(|| grid.shown.iter().position(|shown| of_entry(&shown)))
        })
        .or((!grid.shown.is_empty()).then_some(0));

    // The grid has the keyboard when one of its entries has focus, or when
    // a screen reader asks to focus one.
    let asked = ui.input(|input| {
        grid.shown
            .iter()
            .position(|shown| input.has_accesskit_action_request(entry_id(shown), Action::Focus))
    });
    let focused = asked.or_else(|| {
        let focused = ui.memory(|memory| memory.focused())?;
        grid.shown
            .iter()
            .position(|shown| entry_id(shown) == focused)
    });
    let mut keyboard = focused;
    let mut moved = asked.is_some();
    let mut activate = None;
    if let Some(mut current) = focused {
        let last = grid.shown.len() - 1;
        for key in take_keys(ui, true) {
            match key {
                ListKey::Up | ListKey::Left => current = current.saturating_sub(1),
                ListKey::Down | ListKey::Right => current = (current + 1).min(last),
                ListKey::Home => current = 0,
                ListKey::End => current = last,
                ListKey::Activate => activate = Some(current),
                ListKey::Select | ListKey::Menu => {}
            }
        }
        moved |= Some(current) != focused;
        keyboard = Some(current);
    }
    let mut select = keyboard.filter(|_| moved);

    let top_left = ui.cursor().min;
    let width = ui.available_width();
    let rows = match mode {
        CalendarMode::Month => month(ui, grid, top_left, width),
        CalendarMode::Week | CalendarMode::Day => hours(ui, grid, top_left, width),
    };
    let whole = rows
        .iter()
        .fold(Rect::NOTHING, |whole, row| whole.union(row.rect));
    ui.allocate_rect(whole, Sense::hover());

    let palette = Palette::of(ui.ctx());
    let selected_entry = state.selected.as_ref().map(|(id, _)| id.as_str());
    let mut clicked = None;
    let mut double_clicked = None;
    let mut gained = None;
    for (row_index, row) in rows.iter().enumerate() {
        let builder = UiBuilder::new()
            .id_salt(("row", row_index))
            .max_rect(row.rect)
            .accessibility_role(Role::Row);
        let mut row_ui = ui.new_child(builder);
        for (cell_index, cell) in row.cells.iter().enumerate() {
            let builder = UiBuilder::new()
                .id_salt(("cell", cell_index))
                .max_rect(cell.rect)
                .accessibility_role(cell.role)
                .accessibility_label(cell.label.as_str());
            let cell_ui = row_ui.new_child(builder);
            if cell.today {
                cell_ui
                    .ctx()
                    .accesskit_node_builder(cell_ui.unique_id(), |node| {
                        node.set_aria_current(AriaCurrent::Date);
                    });
            }
            for &(index, rect) in &cell.entries {
                let shown = &grid.shown[index];
                let id = entry_id(shown);
                let sense = if Some(index) == tab_stop {
                    Sense::click()
                } else {
                    Sense::CLICK
                };
                let response = cell_ui.interact(rect, id, sense);
                response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &shown.label));
                let is_selected = selected_entry == Some(grid.ids[shown.entry]);
                cell_ui.ctx().accesskit_node_builder(id, |node| {
                    node.add_action(Action::Focus);
                    node.set_selected(is_selected);
                });
                paint_entry(
                    &cell_ui,
                    rect,
                    &response,
                    grid,
                    shown,
                    is_selected,
                    &palette,
                );
                if response.clicked() {
                    clicked = Some(index);
                }
                if response.double_clicked() {
                    double_clicked = Some(index);
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
                    cell_ui.memory_mut(|memory| memory.set_focus_lock_filter(id, key_filter(true)));
                }
            }
        }
    }

    if let Some(index) = clicked.or(gained) {
        select = Some(index);
    }
    if let Some(index) = clicked {
        ui.memory_mut(|memory| memory.request_focus(entry_id(&grid.shown[index])));
    }
    if let Some(index) = select {
        let shown = &grid.shown[index];
        let entry = grid.ids[shown.entry];
        if selected_entry != Some(entry) {
            kit.events.push(PluginEvent::Selected(Target {
                node: node.id.clone(),
                item: entry.to_owned(),
            }));
        }
        state.selected = Some((entry.to_owned(), grid.days[shown.day].date));
    }
    for index in [activate, double_clicked].into_iter().flatten() {
        let entry = grid.ids[grid.shown[index].entry];
        if state.selected.as_ref().map(|(id, _)| id.as_str()) == Some(entry) {
            kit.events.push(PluginEvent::Activated(Target {
                node: node.id.clone(),
                item: entry.to_owned(),
            }));
        }
    }
    ui.data_mut(|data| data.insert_temp(grid_id, state));
}

/// A month: a row of weekday headers, then six weeks of days, each with
/// its number and its entries stacked under it.
fn month(ui: &Ui, grid: &Grid, top_left: egui::Pos2, width: f32) -> Vec<Row> {
    let palette = Palette::of(ui.ctx());
    let painter = ui.painter();
    let column = width / 7.0;
    let left = |index: usize| top_left.x + index as f32 * column;
    let mut rows = Vec::new();

    let header = Rect::from_min_size(top_left, vec2(width, MONTH_HEADER_HEIGHT));
    painter.rect_filled(header, radius(0.0), palette.muted);
    let cells = grid.days[..7]
        .iter()
        .enumerate()
        .map(|(index, day)| {
            let rect = Rect::from_min_size(
                pos2(left(index), header.top()),
                vec2(column, MONTH_HEADER_HEIGHT),
            );
            let name = text(
                ui,
                &grid.names.weekday(day.date).to_uppercase(),
                column - 16.0,
                SUPPORTING_SIZE,
            );
            painter.galley(
                Align2::LEFT_CENTER
                    .anchor_size(pos2(rect.left() + 8.0, rect.center().y), name.size())
                    .min,
                name,
                palette.muted_foreground,
            );
            Cell {
                rect,
                role: Role::ColumnHeader,
                label: grid.names.weekday_long(day.date),
                today: false,
                entries: Vec::new(),
            }
        })
        .collect();
    rows.push(Row {
        rect: header,
        cells,
    });

    let mut top = header.bottom();
    for week in 0..grid.days.len() / 7 {
        let days = week * 7..week * 7 + 7;
        let most = days
            .clone()
            .map(|day| grid.shown.iter().filter(|shown| shown.day == day).count())
            .max()
            .unwrap_or(0);
        let height = (DAY_NUMBER_HEIGHT + most as f32 * (ENTRY_HEIGHT + ENTRY_GAP) + 6.0)
            .max(MIN_WEEK_HEIGHT);
        let rect = Rect::from_min_size(pos2(top_left.x, top), vec2(width, height));
        let cells = days
            .map(|day_index| {
                let day = &grid.days[day_index];
                let rect =
                    Rect::from_min_size(pos2(left(day_index % 7), top), vec2(column, height));
                let in_month = day.date.month() == grid.focus.month();
                if !in_month {
                    painter.rect_filled(rect, radius(0.0), palette.muted);
                }
                painter.rect_stroke(
                    rect,
                    radius(0.0),
                    Stroke::new(1.0, palette.border),
                    StrokeKind::Inside,
                );
                day_number(
                    ui,
                    grid,
                    day.date,
                    pos2(rect.left() + 18.0, top + 16.0),
                    in_month,
                );
                let mut y = top + DAY_NUMBER_HEIGHT;
                let entries = grid
                    .shown
                    .iter()
                    .enumerate()
                    .filter(|(_, shown)| shown.day == day_index)
                    .map(|(index, _)| {
                        let entry = Rect::from_min_size(
                            pos2(rect.left() + 4.0, y),
                            vec2(column - 8.0, ENTRY_HEIGHT),
                        );
                        y += ENTRY_HEIGHT + ENTRY_GAP;
                        (index, entry)
                    })
                    .collect();
                Cell {
                    rect,
                    role: Role::Cell,
                    label: grid.names.full_date(day.date),
                    today: day.date == grid.today,
                    entries,
                }
            })
            .collect();
        rows.push(Row { rect, cells });
        top += height;
    }
    rows
}

/// A week or a day: a header per day, a row of all-day entries, then a row
/// per hour, with each timed entry placed by its time and lane.
fn hours(ui: &Ui, grid: &Grid, top_left: egui::Pos2, width: f32) -> Vec<Row> {
    let palette = Palette::of(ui.ctx());
    let painter = ui.painter();
    let count = grid.days.len();
    let width = if count > 1 {
        width.max(GUTTER + MIN_WEEK_DAY_WIDTH * count as f32)
    } else {
        width
    };
    let column = (width - GUTTER) / count as f32;
    let left = |index: usize| top_left.x + GUTTER + index as f32 * column;
    let mut rows = Vec::new();

    // The header: each day's weekday and number.
    let header = Rect::from_min_size(top_left, vec2(width, HEADER_HEIGHT));
    painter.rect_filled(header, radius(0.0), palette.muted);
    let cells = grid
        .days
        .iter()
        .enumerate()
        .map(|(index, day)| {
            let rect =
                Rect::from_min_size(pos2(left(index), header.top()), vec2(column, HEADER_HEIGHT));
            let name = text(
                ui,
                &grid.names.weekday(day.date),
                column / 2.0,
                SUPPORTING_SIZE,
            );
            let center = rect.center();
            let name_at =
                Align2::RIGHT_CENTER.anchor_size(pos2(center.x - 2.0, center.y), name.size());
            painter.galley(name_at.min, name, palette.muted_foreground);
            day_number(ui, grid, day.date, pos2(center.x + 14.0, center.y), true);
            Cell {
                rect,
                role: Role::ColumnHeader,
                label: grid.names.full_date(day.date),
                today: day.date == grid.today,
                entries: Vec::new(),
            }
        })
        .collect();
    rows.push(Row {
        rect: header,
        cells,
    });

    // The all-day row.
    let most = (0..count)
        .map(|day| {
            grid.shown
                .iter()
                .filter(|shown| shown.day == day && shown.segment.is_none())
                .count()
        })
        .max()
        .unwrap_or(0);
    let height = (most as f32 * (ENTRY_HEIGHT + ENTRY_GAP) + 8.0).max(MIN_ALL_DAY_HEIGHT);
    let all_day = Rect::from_min_size(pos2(top_left.x, header.bottom()), vec2(width, height));
    let mut cells = vec![row_header(
        ui,
        Rect::from_min_size(all_day.min, vec2(GUTTER, height)),
        "All day".to_owned(),
        &palette,
    )];
    for (day_index, day) in grid.days.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(left(day_index), all_day.top()), vec2(column, height));
        painter.rect_stroke(
            rect,
            radius(0.0),
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );
        let mut y = rect.top() + 4.0;
        let entries = grid
            .shown
            .iter()
            .enumerate()
            .filter(|(_, shown)| shown.day == day_index && shown.segment.is_none())
            .map(|(index, _)| {
                let entry = Rect::from_min_size(
                    pos2(rect.left() + 4.0, y),
                    vec2(column - 8.0, ENTRY_HEIGHT),
                );
                y += ENTRY_HEIGHT + ENTRY_GAP;
                (index, entry)
            })
            .collect();
        cells.push(Cell {
            rect,
            role: Role::Cell,
            label: grid.names.full_date(day.date),
            today: false,
            entries,
        });
    }
    rows.push(Row {
        rect: all_day,
        cells,
    });

    // A row per hour. Each timed entry sits in the cell of its start's hour
    // and draws down to its end.
    let first_hour = all_day.bottom();
    let day_labels: Vec<String> = grid
        .days
        .iter()
        .map(|day| grid.names.full_date(day.date))
        .collect();
    for hour in 0..24_i8 {
        let top = first_hour + f32::from(hour) * HOUR_HEIGHT;
        let rect = Rect::from_min_size(pos2(top_left.x, top), vec2(width, HOUR_HEIGHT));
        let hour_name = grid.names.hour(hour);
        painter.line_segment(
            [pos2(left(0), top), pos2(rect.right(), top)],
            Stroke::new(1.0, palette.border),
        );
        let mut cells = vec![row_header(
            ui,
            Rect::from_min_size(rect.min, vec2(GUTTER, HOUR_HEIGHT)),
            hour_name.clone(),
            &palette,
        )];
        for (day_index, day) in grid.days.iter().enumerate() {
            let cell = Rect::from_min_size(pos2(left(day_index), top), vec2(column, HOUR_HEIGHT));
            painter.line_segment(
                [cell.left_top(), cell.left_bottom()],
                Stroke::new(1.0, palette.border),
            );
            let entries = grid
                .shown
                .iter()
                .enumerate()
                .filter(|(_, shown)| shown.day == day_index)
                .filter_map(|(index, shown)| {
                    let segment = &day.timed[shown.segment?];
                    let starts = (segment.start_minute / 60).min(23);
                    if starts != u32::from(hour.unsigned_abs()) {
                        return None;
                    }
                    let lane_width = (column - 8.0) / segment.lanes.max(1) as f32;
                    let y = first_hour + segment.start_minute as f32 / 60.0 * HOUR_HEIGHT;
                    let height = ((segment.end_minute - segment.start_minute) as f32 / 60.0
                        * HOUR_HEIGHT)
                        .max(MIN_TIMED_HEIGHT);
                    let entry = Rect::from_min_size(
                        pos2(cell.left() + 4.0 + segment.lane as f32 * lane_width, y),
                        vec2(lane_width - 2.0, height),
                    );
                    Some((index, entry))
                })
                .collect();
            cells.push(Cell {
                rect: cell,
                role: Role::Cell,
                label: format!("{}, {hour_name}", day_labels[day_index]),
                today: false,
                entries,
            });
        }
        rows.push(Row { rect, cells });
    }
    rows
}

/// A row's header, its hour or All day, drawn in the gutter.
fn row_header(ui: &Ui, rect: Rect, label: String, palette: &Palette) -> Cell {
    let galley = text(ui, &label, GUTTER - 8.0, SUPPORTING_SIZE);
    ui.painter().galley(
        Align2::RIGHT_TOP
            .anchor_size(pos2(rect.right() - 6.0, rect.top() + 4.0), galley.size())
            .min,
        galley,
        palette.muted_foreground,
    );
    Cell {
        rect,
        role: Role::RowHeader,
        label,
        today: false,
        entries: Vec::new(),
    }
}

/// A day's number centered on `center`, in a filled circle for today. A
/// day outside the month shows quieter.
fn day_number(ui: &Ui, grid: &Grid, date: Date, center: egui::Pos2, in_month: bool) {
    let palette = Palette::of(ui.ctx());
    let galley = text(ui, &grid.names.day_number(date), 40.0, SUPPORTING_SIZE);
    let color = if date == grid.today {
        ui.painter().circle_filled(center, 12.0, palette.primary);
        palette.primary_foreground
    } else if in_month {
        palette.foreground
    } else {
        palette.muted_foreground
    };
    ui.painter().galley(
        Align2::CENTER_CENTER.anchor_size(center, galley.size()).min,
        galley,
        color,
    );
}

/// An entry's box with its title, and a timed entry's time under it when
/// there is room. A selected entry fills with the primary color.
fn paint_entry(
    ui: &Ui,
    rect: Rect,
    response: &Response,
    grid: &Grid,
    shown: &Shown,
    selected: bool,
    palette: &Palette,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let (fill, foreground, quiet) = if selected {
        (
            palette.primary,
            palette.primary_foreground,
            palette.primary_foreground,
        )
    } else if response.hovered() {
        (palette.accent, palette.foreground, palette.muted_foreground)
    } else {
        (
            palette.secondary,
            palette.foreground,
            palette.muted_foreground,
        )
    };
    painter.rect(
        rect,
        radius(0.4),
        fill,
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect.shrink(1.0),
            radius(0.4),
            Stroke::new(1.5, palette.ring),
            StrokeKind::Inside,
        );
    }
    let width = rect.width() - 12.0;
    let title = text(ui, grid.entries[shown.entry].title, width, SUPPORTING_SIZE);
    let title_height = title.size().y;
    painter.galley(pos2(rect.left() + 6.0, rect.top() + 3.0), title, foreground);
    if shown.segment.is_some() && rect.height() >= title_height * 2.0 + 8.0 {
        let times = shown.label.rsplit_once(", ").map_or("", |(_, times)| times);
        let times = text(ui, times, width, SUPPORTING_SIZE);
        painter.galley(
            pos2(rect.left() + 6.0, rect.top() + 4.0 + title_height),
            times,
            quiet,
        );
    }
}
