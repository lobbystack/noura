//! Calendar grids: a month of six weeks from Sunday, a week from Sunday to
//! Saturday, or a day, laid out as `app-core` places the plugin's entries,
//! in this computer's time zone and locale. A week and a day have a row of
//! all-day entries above a row per hour, where overlapping entries sit
//! side by side.
//!
//! On a page the grid takes the rest of the page, edge to edge, with
//! hairlines between its days and no fills, as Apple's Calendar draws
//! one: a month's six weeks share the height under the weekday names, and
//! a week's or a day's hours scroll on their own under its days and its
//! all-day row. A month's day shows the entries that fit and a line that
//! says how many more it holds; a press on that line, or the keyboard
//! reaching one of them, makes the week tall enough for all of them until
//! the grid shows another period. Elsewhere the grid is as tall as its
//! entries need.
//!
//! AccessKit sees a grid: a row of column headers, then rows of cells, one
//! cell per day, or per day and hour. Each entry sits in the cell of its
//! day, and of its start's hour, labeled with its title and time, whether
//! or not the cell has room to show it. Today's cell in a month, and
//! today's header in a week or a day, carry the current-date state.
//!
//! You select an entry by click or the keyboard and open it with Enter or
//! a double-click. A click selects it even when it already is, so a
//! plugin can show it again. The arrow keys walk the entries day by day, and Home
//! and End go to the first and last. An entry shows once on each day it
//! covers, and the grid is one stop in the Tab order, on the selected
//! entry where the keyboard last left it.

use app_core::{
    CalendarMode, GridDay, GridEntry, PluginColor, PluginEvent, Span, Target, ViewKind, ViewNode,
    grid_days, lay_out,
};
use egui::{
    Align2, Color32, Id, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder,
    WidgetInfo,
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
    theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, TITLE_SIZE, Weight, font, radius},
};

/// The height of an hour in a week or a day, and the hour labels' width.
const HOUR_HEIGHT: f32 = 56.0;
const GUTTER: f32 = 56.0;
/// The height of a week's or a day's header, and of a month's.
const HEADER_HEIGHT: f32 = 40.0;
const MONTH_HEADER_HEIGHT: f32 = 28.0;
/// The least a month's week, or a week's all-day row, is tall: room for
/// a day's number and two lines.
const MIN_WEEK_HEIGHT: f32 = DAY_NUMBER_HEIGHT + 2.0 * (MONTH_ENTRY_HEIGHT + ENTRY_GAP) + 4.0;
const MIN_ALL_DAY_HEIGHT: f32 = 32.0;
/// The height of an entry in a month, and of one in a week's or a day's
/// all-day row, and the space between entries.
const MONTH_ENTRY_HEIGHT: f32 = 18.0;
const ENTRY_HEIGHT: f32 = 22.0;
const ENTRY_GAP: f32 = 2.0;
/// The least a timed entry is tall in a week or a day.
const MIN_TIMED_HEIGHT: f32 = 24.0;
/// The space a day number takes at the top of a month's day.
const DAY_NUMBER_HEIGHT: f32 = 28.0;
/// The narrowest a week's day may be.
const MIN_WEEK_DAY_WIDTH: f32 = 72.0;
/// The least room a week's or a day's hours get on a page: four hours.
const MIN_HOURS_HEIGHT: f32 = 4.0 * HOUR_HEIGHT;
/// The latest hour a week or a day first scrolls to, when no entry starts
/// earlier.
const FIRST_HOUR: u32 = 8;
/// How far the grid runs past the page's 24 point margins, to its edges.
const PAGE_MARGIN: f32 = 24.0;

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
    /// The entries the cell shows, by index into the shown entries, and
    /// where each one draws.
    entries: Vec<(usize, Rect)>,
    /// The entries a month's day holds past the ones it has room for, by
    /// index into the shown entries, and the line that says how many.
    more: Option<More>,
}

/// The entries a month's day has no room for.
struct More {
    /// The line that says how many, which a press expands the week at.
    rect: Rect,
    /// The week the day is in.
    week: usize,
    entries: Vec<usize>,
}

impl Cell {
    fn new(rect: Rect, role: Role, label: String) -> Self {
        Self {
            rect,
            role,
            label,
            today: false,
            entries: Vec::new(),
            more: None,
        }
    }
}

/// A row of the grid.
struct Row {
    rect: Rect,
    cells: Vec<Cell>,
}

/// What a calendar grid remembers between frames, in egui's memory under
/// its ID.
#[derive(Clone, Default)]
struct State {
    /// The selected entry's node ID and the day the keyboard left it on.
    selected: Option<(String, Date)>,
    /// A month's week tall enough for all its entries, by its index, and
    /// the grid's first day, which it holds for.
    expanded: Option<(Date, usize)>,
    /// The first day and the number of days of the week or the day whose
    /// hours last scrolled to its first entry.
    scrolled: Option<(Date, usize)>,
}

/// The days, the entries and the time zone of one grid.
struct Grid<'a> {
    mode: CalendarMode,
    names: &'a DateNames,
    time_zone: TimeZone,
    today: Date,
    focus: Date,
    days: Vec<GridDay>,
    entries: Vec<GridEntry<'a>>,
    ids: Vec<&'a str>,
    /// Each entry's color, when the plugin gave one.
    colors: Vec<Option<PluginColor>>,
    shown: Vec<Shown>,
}

impl Grid<'_> {
    fn local(&self, at: Timestamp) -> jiff::civil::Time {
        at.to_zoned(self.time_zone.clone()).time()
    }

    /// The entries on the day `day`, by index into the shown entries:
    /// all-day ones, then timed ones as they start.
    fn on_day(&self, day: usize) -> impl Iterator<Item = usize> + '_ {
        self.shown
            .iter()
            .enumerate()
            .filter(move |(_, shown)| shown.day == day)
            .map(|(index, _)| index)
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
    let mut colors = Vec::new();
    let mut entries = Vec::new();
    for child in &node.children {
        if let ViewKind::Entry {
            title,
            start,
            end,
            color,
        } = &child.kind
            && let Some(span) = Span::parse(start, end.as_deref())
        {
            ids.push(child.id.as_str());
            colors.push(*color);
            entries.push(GridEntry { title, span });
        }
    }
    let dates = grid_days(mode, focus);
    let days = lay_out(&entries, &dates, &time_zone);
    let mut grid = Grid {
        mode,
        names: &names,
        time_zone,
        today: kit.app.today(),
        focus,
        days,
        entries,
        ids,
        colors,
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
    // A period heading names the period already, so the grid adds no
    // heading. Under a page heading whose detail names the month, as the
    // legacy page header did, the grid's own heading only names it for
    // screen readers, and a day keeps its date over its hours.
    let unseen = !kit.page_period && kit.page_detail && mode != CalendarMode::Day;
    if !kit.page_period {
        ui.scope_builder(builder, |ui| {
            if unseen {
                let (_, response) = ui.allocate_exact_size(vec2(0.0, 0.0), egui::Sense::hover());
                response.widget_info(|| egui::WidgetInfo::labeled(Role::Label, true, &heading));
            } else {
                ui.label(RichText::new(&heading).font(font(
                    ui.ctx(),
                    TITLE_SIZE,
                    Weight::Semibold,
                )));
            }
        });
    }
    // Under a page header that names the period, the grid starts just
    // under the header's line, over the gaps above it and its unseen
    // title, and leaves the line itself as on every other page.
    let gaps = if kit.page_period {
        1.0
    } else if unseen {
        2.0
    } else {
        0.0
    };
    let lift = (gaps * ui.spacing().item_spacing.y - 1.0).max(0.0);
    let builder = UiBuilder::new()
        .accessibility_role(Role::Grid)
        .accessibility_label(label);
    ui.scope_builder(builder, |ui| draw(ui, node, mode, lift, &grid, kit));
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

/// What the grid's entries need this frame to draw and answer input: the
/// keyboard's place and the selection.
struct Input<'a> {
    grid_id: Id,
    /// The entry that takes the grid's stop in the Tab order.
    tab_stop: Option<usize>,
    /// The entry the keyboard is on, and whether it moved there this
    /// frame.
    keyboard: Option<usize>,
    moved: bool,
    selected: Option<&'a (String, Date)>,
}

impl Input<'_> {
    fn entry_id(&self, grid: &Grid, shown: &Shown) -> Id {
        entry_id(self.grid_id, grid, shown)
    }
}

fn entry_id(grid_id: Id, grid: &Grid, shown: &Shown) -> Id {
    grid_id.with((
        "entry",
        grid.ids[shown.entry],
        grid.days[shown.day].date.to_string(),
    ))
}

/// What happened to the grid's entries this frame.
#[derive(Default)]
struct Outcome {
    clicked: Option<usize>,
    double_clicked: Option<usize>,
    gained: Option<usize>,
    /// A month's week to make tall enough for all its entries.
    expand: Option<usize>,
}

/// Draw the grid `lift` points above the cursor, over the gaps under a
/// page header.
fn draw(ui: &mut Ui, node: &ViewNode, mode: CalendarMode, lift: f32, grid: &Grid, kit: &mut Kit) {
    let grid_id = kit.id(&node.id);
    let mut state: State = ui.data(|data| data.get_temp(grid_id)).unwrap_or_default();
    state.selected = state
        .selected
        .take()
        .filter(|(id, _)| grid.ids.contains(&id.as_str()));
    let first_day = grid.days.first().map_or(grid.focus, |day| day.date);
    state.expanded = state.expanded.filter(|(day, _)| *day == first_day);
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
        grid.shown.iter().position(|shown| {
            input.has_accesskit_action_request(entry_id(grid_id, grid, shown), Action::Focus)
        })
    });
    let focused = asked.or_else(|| {
        let focused = ui.memory(|memory| memory.focused())?;
        grid.shown
            .iter()
            .position(|shown| entry_id(grid_id, grid, shown) == focused)
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

    // On a page the grid runs past the page's margins to its edges, and
    // down to its bottom.
    let page = matches!(kit.place, super::Place::Page(_));
    let bleed = if page { PAGE_MARGIN } else { 0.0 };
    let lift = if page { lift } else { 0.0 };
    let top_left = ui.cursor().min - vec2(bleed, lift);
    let width = ui.available_width() + 2.0 * bleed;
    // The page's own bottom, where it rests unscrolled, so scrolling the
    // page never makes the grid taller.
    let bottom = page.then(|| ui.max_rect().bottom() + bleed);
    let area = Area {
        top_left,
        width,
        bottom,
    };

    let selected = state.selected.clone();
    let input = Input {
        grid_id,
        tab_stop,
        keyboard,
        moved,
        selected: selected.as_ref(),
    };
    let mut outcome = Outcome::default();
    let whole = match mode {
        CalendarMode::Month => {
            let expanded = state.expanded.map(|(_, week)| week);
            let rows = month(ui, grid, &area, expanded);
            draw_cells(ui, &rows, 0, grid, &input, kit, &mut outcome);
            rows.iter()
                .fold(Rect::NOTHING, |whole, row| whole.union(row.rect))
        }
        CalendarMode::Week | CalendarMode::Day => {
            hours(ui, grid, &area, &input, &mut state, kit, &mut outcome)
        }
    };
    // The part in the page's margins doesn't widen or lengthen the page.
    let mut inside = whole.shrink2(vec2(bleed, 0.0));
    inside.max.y = (inside.max.y - bleed).max(inside.min.y);
    ui.allocate_rect(inside, Sense::hover());

    let Outcome {
        clicked,
        double_clicked,
        gained,
        expand,
    } = outcome;
    if let Some(week) = expand {
        state.expanded = Some((first_day, week));
    }
    if let Some(index) = clicked.or(gained) {
        select = Some(index);
    }
    if let Some(index) = clicked {
        ui.memory_mut(|memory| {
            memory.request_focus(entry_id(grid_id, grid, &grid.shown[index]));
        });
    }
    let selected_entry = selected.as_ref().map(|(id, _)| id.as_str());
    if let Some(index) = select {
        let shown = &grid.shown[index];
        let entry = grid.ids[shown.entry];
        // A click on the selected entry selects it again, so its popover
        // opens again after you closed it.
        if selected_entry != Some(entry) || clicked.is_some() {
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

/// Where the grid draws: from its top left corner, this wide, and on a
/// page down to the page's bottom.
struct Area {
    top_left: Pos2,
    width: f32,
    bottom: Option<f32>,
}

/// Draw the entries of `rows`, whose first row is the grid's row
/// `first_row`, as AccessKit's rows and cells, and note what happened to
/// them.
fn draw_cells(
    ui: &mut Ui,
    rows: &[Row],
    first_row: usize,
    grid: &Grid,
    input: &Input,
    kit: &mut Kit,
    outcome: &mut Outcome,
) {
    let palette = Palette::of(ui.ctx());
    let selected_entry = input.selected.map(|(id, _)| id.as_str());
    for (row_index, row) in rows.iter().enumerate() {
        let builder = UiBuilder::new()
            .id_salt(("row", first_row + row_index))
            .max_rect(row.rect)
            .accessibility_role(Role::Row);
        let mut row_ui = ui.new_child(builder);
        bounds(&row_ui, row.rect);
        for (cell_index, cell) in row.cells.iter().enumerate() {
            let builder = UiBuilder::new()
                .id_salt(("cell", cell_index))
                .max_rect(cell.rect)
                .accessibility_role(cell.role)
                .accessibility_label(cell.label.as_str());
            let cell_ui = row_ui.new_child(builder);
            bounds(&cell_ui, cell.rect);
            if cell.today {
                cell_ui
                    .ctx()
                    .accesskit_node_builder(cell_ui.unique_id(), |node| {
                        node.set_aria_current(AriaCurrent::Date);
                    });
            }
            // An entry with no room doesn't draw, but screen readers and
            // the keyboard still reach it, at the line that counts it,
            // which comes after it and so takes the pointer's presses.
            let hidden = cell.more.iter().flat_map(|more| {
                more.entries
                    .iter()
                    .map(move |&index| (index, more.rect, false))
            });
            let entries = cell
                .entries
                .iter()
                .map(|&(index, rect)| (index, rect, true))
                .chain(hidden);
            for (index, rect, visible) in entries {
                let shown = &grid.shown[index];
                let id = input.entry_id(grid, shown);
                let sense = if Some(index) == input.tab_stop {
                    Sense::click()
                } else {
                    Sense::CLICK
                };
                let response = cell_ui.interact(rect, id, sense);
                response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &shown.label));
                let is_selected = selected_entry == Some(grid.ids[shown.entry]);
                // A popover points at the entry on the day you selected
                // it, or on its first day.
                let selected_day = input
                    .selected
                    .is_some_and(|(_, day)| *day == grid.days[shown.day].date);
                kit.anchor(grid.ids[shown.entry], rect, is_selected && selected_day);
                cell_ui.ctx().accesskit_node_builder(id, |node| {
                    node.add_action(Action::Focus);
                    node.set_selected(is_selected);
                });
                if visible {
                    paint_entry(
                        &cell_ui,
                        rect,
                        &response,
                        grid,
                        shown,
                        is_selected,
                        &palette,
                    );
                }
                if response.clicked() {
                    outcome.clicked = Some(index);
                }
                if response.double_clicked() {
                    outcome.double_clicked = Some(index);
                }
                let arrived = Some(index) == input.keyboard && input.moved;
                if response.gained_focus() {
                    outcome.gained = Some(index);
                    response.scroll_to_me(None);
                }
                if arrived && !response.has_focus() {
                    response.request_focus();
                    response.scroll_to_me(None);
                }
                if !visible && (arrived || response.gained_focus()) {
                    outcome.expand = cell.more.as_ref().map(|more| more.week);
                }
                if response.has_focus() {
                    cell_ui.memory_mut(|memory| memory.set_focus_lock_filter(id, key_filter(true)));
                }
            }
            if let Some(more) = &cell.more {
                let label = format!("+{} more", more.entries.len());
                let id = input.grid_id.with(("more", cell.label.as_str()));
                let response = cell_ui.interact(more.rect, id, Sense::CLICK);
                response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
                let color = if response.hovered() {
                    palette.foreground
                } else {
                    palette.muted_foreground
                };
                let galley = text(&cell_ui, &label, more.rect.width() - 8.0, SUPPORTING_SIZE);
                cell_ui.painter().galley(
                    Align2::LEFT_CENTER
                        .anchor_size(
                            pos2(more.rect.left() + 4.0, more.rect.center().y),
                            galley.size(),
                        )
                        .min,
                    galley,
                    color,
                );
                if response.clicked() {
                    outcome.expand = Some(more.week);
                }
            }
        }
    }
}

/// Tell screen readers that the row or cell `ui` sits at `rect`, as a
/// widget's own node does.
fn bounds(ui: &Ui, rect: Rect) {
    ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
        node.set_bounds(egui::accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
}

/// A hairline in the grid's color.
fn hairline(palette: &Palette) -> Stroke {
    Stroke::new(1.0, palette.border)
}

/// The x of the left edge of each of `count` columns sharing `width` from
/// `left`, then the right edge, whole points so the lines stay sharp.
fn columns(left: f32, width: f32, count: usize) -> Vec<f32> {
    (0..=count)
        .map(|index| (left + width * index as f32 / count as f32).round())
        .collect()
}

/// A month: a row of weekday names, then six weeks of days, each with
/// its number and as many of its entries as fit under it. On a page the
/// weeks share the page's height; `expanded` is a week as tall as all
/// its entries need.
fn month(ui: &Ui, grid: &Grid, area: &Area, expanded: Option<usize>) -> Vec<Row> {
    let palette = Palette::of(ui.ctx());
    let painter = ui.painter();
    let top_left = area.top_left;
    let width = area.width;
    let xs = columns(top_left.x, width, 7);
    let mut rows = Vec::new();

    let header = Rect::from_min_size(top_left, vec2(width, MONTH_HEADER_HEIGHT));
    let cells = grid.days[..7]
        .iter()
        .enumerate()
        .map(|(index, day)| {
            let rect = Rect::from_min_max(
                pos2(xs[index], header.top()),
                pos2(xs[index + 1], header.bottom()),
            );
            // Each name sits over its days' numbers, at the column's end.
            let name = text(
                ui,
                &grid.names.weekday(day.date),
                rect.width() - 16.0,
                BODY_SIZE,
            );
            painter.galley(
                Align2::RIGHT_CENTER
                    .anchor_size(pos2(rect.right() - 8.0, rect.center().y), name.size())
                    .min,
                name,
                palette.muted_foreground,
            );
            Cell::new(rect, Role::ColumnHeader, grid.names.weekday_long(day.date))
        })
        .collect();
    rows.push(Row {
        rect: header,
        cells,
    });

    let weeks = grid.days.len() / 7;
    let needed = |week: usize| {
        let most = (week * 7..week * 7 + 7)
            .map(|day| grid.on_day(day).count())
            .max()
            .unwrap_or(0);
        DAY_NUMBER_HEIGHT + most as f32 * (MONTH_ENTRY_HEIGHT + ENTRY_GAP) + 4.0
    };
    // Each week's share of the page, or, off a page, what its entries
    // need.
    let share = area.bottom.map(|bottom| {
        ((bottom - header.bottom()) / weeks as f32)
            .floor()
            .max(MIN_WEEK_HEIGHT)
    });
    painter.hline(header.x_range(), header.bottom(), hairline(&palette));
    let mut top = header.bottom();
    let mut week_rects = Vec::new();
    for week in 0..weeks {
        let mut height = match share {
            Some(share) if Some(week) == expanded => share.max(needed(week)),
            Some(share) => share,
            None => needed(week).max(MIN_WEEK_HEIGHT),
        };
        // The last week takes what rounding left over, down to the bottom.
        if let (Some(bottom), true) = (area.bottom, week + 1 == weeks) {
            height = height.max(bottom - top);
        }
        week_rects.push(Rect::from_min_size(
            pos2(top_left.x, top),
            vec2(width, height),
        ));
        top += height;
    }
    for x in &xs[1..7] {
        painter.vline(*x, header.bottom()..=top, hairline(&palette));
    }

    for (week, rect) in week_rects.into_iter().enumerate() {
        if week > 0 {
            painter.hline(rect.x_range(), rect.top(), hairline(&palette));
        }
        let cells = (week * 7..week * 7 + 7)
            .map(|day_index| {
                let column = day_index % 7;
                let cell = Rect::from_min_max(
                    pos2(xs[column], rect.top()),
                    pos2(xs[column + 1], rect.bottom()),
                );
                month_day(ui, grid, day_index, cell, week, share.is_some())
            })
            .collect();
        rows.push(Row { rect, cells });
    }
    rows
}

/// A month's day: its number at its top right, then its entries, as many
/// as fit when `fitted`, and a line that counts the rest.
fn month_day(
    ui: &Ui,
    grid: &Grid,
    day_index: usize,
    rect: Rect,
    week: usize,
    fitted: bool,
) -> Cell {
    let day = &grid.days[day_index];
    let in_month = day.date.month() == grid.focus.month();
    let number = if day.date.day() == 1 {
        grid.names.month_day(day.date)
    } else {
        grid.names.day_number(day.date)
    };
    day_number(
        ui,
        grid,
        day.date,
        &number,
        pos2(rect.right() - 8.0, rect.top() + DAY_NUMBER_HEIGHT / 2.0),
        Align2::RIGHT_CENTER,
        in_month,
    );
    let on_day: Vec<usize> = grid.on_day(day_index).collect();
    let line = MONTH_ENTRY_HEIGHT + ENTRY_GAP;
    let room = if fitted {
        ((rect.height() - DAY_NUMBER_HEIGHT - 4.0) / line)
            .floor()
            .max(0.0) as usize
    } else {
        on_day.len()
    };
    let (fits, rest) = if on_day.len() <= room {
        (on_day.as_slice(), &[][..])
    } else {
        on_day.split_at(room.saturating_sub(1))
    };
    let mut y = rect.top() + DAY_NUMBER_HEIGHT;
    let mut place = || {
        let at = Rect::from_min_size(
            pos2(rect.left() + 4.0, y),
            vec2(rect.width() - 8.0, MONTH_ENTRY_HEIGHT),
        );
        y += line;
        at
    };
    let entries = fits.iter().map(|&index| (index, place())).collect();
    let more = (!rest.is_empty()).then(|| More {
        rect: place(),
        week,
        entries: rest.to_vec(),
    });
    let mut cell = Cell::new(rect, Role::Cell, grid.names.full_date(day.date));
    cell.today = day.date == grid.today;
    cell.entries = entries;
    cell.more = more;
    cell
}

/// A week or a day: a header per day and a row of all-day entries, then a
/// row per hour, with each timed entry placed by its time and lane. On a
/// page the hours scroll on their own down to the page's bottom, and
/// first to the hour of the period's earliest entry, or 8 AM. Returns the
/// grid's whole rectangle.
fn hours(
    ui: &mut Ui,
    grid: &Grid,
    area: &Area,
    input: &Input,
    state: &mut State,
    kit: &mut Kit,
    outcome: &mut Outcome,
) -> Rect {
    let palette = Palette::of(ui.ctx());
    let count = grid.days.len();
    let width = if count > 1 {
        area.width.max(GUTTER + MIN_WEEK_DAY_WIDTH * count as f32)
    } else {
        area.width
    };
    let top_left = area.top_left;
    let xs = columns(top_left.x + GUTTER, width - GUTTER, count);
    let mut rows = Vec::new();
    let painter = ui.painter().clone();

    // The header: each day's weekday and number.
    let header = Rect::from_min_size(top_left, vec2(width, HEADER_HEIGHT));
    let cells = grid
        .days
        .iter()
        .enumerate()
        .map(|(index, day)| {
            let rect = Rect::from_min_max(
                pos2(xs[index], header.top()),
                pos2(xs[index + 1], header.bottom()),
            );
            let name = text(
                ui,
                &grid.names.weekday(day.date),
                rect.width() / 2.0,
                BODY_SIZE,
            );
            let center = rect.center();
            let name_at =
                Align2::RIGHT_CENTER.anchor_size(pos2(center.x - 2.0, center.y), name.size());
            painter.galley(name_at.min, name, palette.muted_foreground);
            day_number(
                ui,
                grid,
                day.date,
                &grid.names.day_number(day.date),
                pos2(center.x + 2.0, center.y),
                Align2::LEFT_CENTER,
                true,
            );
            let mut cell = Cell::new(rect, Role::ColumnHeader, grid.names.full_date(day.date));
            cell.today = day.date == grid.today;
            cell
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
    let height = (most as f32 * (ENTRY_HEIGHT + ENTRY_GAP) + 6.0).max(MIN_ALL_DAY_HEIGHT);
    let all_day = Rect::from_min_size(pos2(top_left.x, header.bottom()), vec2(width, height));
    painter.hline(header.x_range(), header.bottom(), hairline(&palette));
    painter.hline(all_day.x_range(), all_day.bottom(), hairline(&palette));
    let mut cells = vec![row_header(
        ui,
        Rect::from_min_size(all_day.min, vec2(GUTTER, height)),
        "All day".to_owned(),
        &palette,
        false,
    )];
    for (day_index, day) in grid.days.iter().enumerate() {
        let rect = Rect::from_min_max(
            pos2(xs[day_index], all_day.top()),
            pos2(xs[day_index + 1], all_day.bottom()),
        );
        painter.vline(rect.left(), rect.y_range(), hairline(&palette));
        let mut y = rect.top() + 3.0;
        let mut cell = Cell::new(rect, Role::Cell, grid.names.full_date(day.date));
        cell.entries = grid
            .shown
            .iter()
            .enumerate()
            .filter(|(_, shown)| shown.day == day_index && shown.segment.is_none())
            .map(|(index, _)| {
                let entry = Rect::from_min_size(
                    pos2(rect.left() + 4.0, y),
                    vec2(rect.width() - 8.0, ENTRY_HEIGHT),
                );
                y += ENTRY_HEIGHT + ENTRY_GAP;
                (index, entry)
            })
            .collect();
        cells.push(cell);
    }
    rows.push(Row {
        rect: all_day,
        cells,
    });
    draw_cells(ui, &rows, 0, grid, input, kit, outcome);

    // The hours, under the all-day row: on a page, down to its bottom.
    let day_height = 24.0 * HOUR_HEIGHT;
    let shown_height = area.bottom.map_or(day_height, |bottom| {
        (bottom - all_day.bottom()).clamp(MIN_HOURS_HEIGHT, day_height)
    });
    let viewport = Rect::from_min_size(
        pos2(top_left.x, all_day.bottom() + 1.0),
        vec2(width, shown_height - 1.0),
    );
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt((input.grid_id, "hours"))
        .auto_shrink(false)
        .max_height(viewport.height());
    let period = (grid.days[0].date, count);
    if state.scrolled != Some(period) {
        state.scrolled = Some(period);
        let earliest = grid
            .days
            .iter()
            .flat_map(|day| &day.timed)
            .map(|segment| segment.start_minute / 60)
            .min()
            .map_or(FIRST_HOUR, |hour| hour.min(FIRST_HOUR));
        // A little above the hour, so its name on its line shows whole.
        scroll = scroll.vertical_scroll_offset((earliest as f32 * HOUR_HEIGHT - 12.0).max(0.0));
    }
    let mut hours_ui = ui.new_child(
        UiBuilder::new()
            .id_salt("hours")
            .max_rect(viewport)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    scroll.show(&mut hours_ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(width, day_height), Sense::hover());
        let rows = hour_rows(ui, grid, rect.min, &xs, width);
        draw_cells(ui, &rows, 2, grid, input, kit, outcome);
    });
    Rect::from_min_max(top_left, pos2(top_left.x + width, viewport.bottom()))
}

/// A row per hour from `top_left`, with the days' columns at `xs`. Each
/// timed entry sits in the cell of its start's hour and draws down to its
/// end.
fn hour_rows(ui: &Ui, grid: &Grid, top_left: Pos2, xs: &[f32], width: f32) -> Vec<Row> {
    let palette = Palette::of(ui.ctx());
    let painter = ui.painter();
    let first_hour = top_left.y;
    let day_labels: Vec<String> = grid
        .days
        .iter()
        .map(|day| grid.names.full_date(day.date))
        .collect();
    for x in &xs[..xs.len() - 1] {
        painter.vline(
            *x,
            first_hour..=first_hour + 24.0 * HOUR_HEIGHT,
            hairline(&palette),
        );
    }
    let mut rows = Vec::new();
    for hour in 0..24_i8 {
        let top = (first_hour + f32::from(hour) * HOUR_HEIGHT).round();
        let rect = Rect::from_min_size(pos2(top_left.x, top), vec2(width, HOUR_HEIGHT));
        let hour_name = grid.names.hour(hour);
        if hour > 0 {
            painter.hline(xs[0]..=rect.right(), top, hairline(&palette));
        }
        let mut cells = vec![row_header(
            ui,
            Rect::from_min_size(rect.min, vec2(GUTTER, HOUR_HEIGHT)),
            hour_name.clone(),
            &palette,
            hour > 0,
        )];
        for (day_index, day) in grid.days.iter().enumerate() {
            let cell = Rect::from_min_max(
                pos2(xs[day_index], top),
                pos2(xs[day_index + 1], top + HOUR_HEIGHT),
            );
            let mut hour_cell = Cell::new(
                cell,
                Role::Cell,
                format!("{}, {hour_name}", day_labels[day_index]),
            );
            hour_cell.entries = grid
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
                    let column = cell.width();
                    let lane_width = (column - 6.0) / segment.lanes.max(1) as f32;
                    let y = first_hour + segment.start_minute as f32 / 60.0 * HOUR_HEIGHT;
                    let height = ((segment.end_minute - segment.start_minute) as f32 / 60.0
                        * HOUR_HEIGHT)
                        .max(MIN_TIMED_HEIGHT);
                    let entry = Rect::from_min_size(
                        pos2(
                            cell.left() + 3.0 + segment.lane as f32 * lane_width,
                            y + 1.0,
                        ),
                        vec2(lane_width - 2.0, height - 2.0),
                    );
                    Some((index, entry))
                })
                .collect();
            cells.push(hour_cell);
        }
        rows.push(Row { rect, cells });
    }
    rows
}

/// A row's header, its hour or All day, drawn in the gutter: an hour's
/// name on its line, as `on_line` asks, and the first hour's and All
/// day's at the top.
fn row_header(ui: &Ui, rect: Rect, label: String, palette: &Palette, on_line: bool) -> Cell {
    let galley = text(ui, &label, GUTTER - 8.0, SUPPORTING_SIZE);
    let at = if on_line {
        Align2::RIGHT_CENTER.anchor_size(pos2(rect.right() - 8.0, rect.top()), galley.size())
    } else {
        Align2::RIGHT_TOP.anchor_size(pos2(rect.right() - 8.0, rect.top() + 4.0), galley.size())
    };
    ui.painter()
        .galley(at.min, galley, palette.muted_foreground);
    Cell::new(rect, Role::RowHeader, label)
}

/// A day's number `number` at `at`, set by `align`, in a filled red
/// circle for today, or a pill when the number is long, such as `Nov 1`.
/// A day outside the month shows quieter.
fn day_number(
    ui: &Ui,
    grid: &Grid,
    date: Date,
    number: &str,
    at: Pos2,
    align: Align2,
    in_month: bool,
) {
    let palette = Palette::of(ui.ctx());
    let today = date == grid.today;
    let weight = if today {
        Weight::Semibold
    } else {
        Weight::Regular
    };
    let font = font(ui.ctx(), BODY_SIZE, weight);
    let galley =
        ui.fonts_mut(|fonts| fonts.layout_no_wrap(number.to_owned(), font, Color32::PLACEHOLDER));
    // A marker 24 points tall, its number 7 points in from each end.
    let marker = vec2((galley.size().x + 14.0).max(24.0), 24.0);
    let marker = align.anchor_size(at, marker);
    let color = if today {
        ui.painter()
            .rect_filled(marker, egui::CornerRadius::same(12), palette.destructive);
        Color32::WHITE
    } else if in_month {
        palette.foreground
    } else {
        palette.muted_foreground
    };
    ui.painter().galley(
        Align2::CENTER_CENTER
            .anchor_size(marker.center(), galley.size())
            .min,
        galley,
        color,
    );
}

/// An entry as its place draws it: in a month, a timed entry is a dot in
/// its color before its title; an all-day entry is a pill tinted with its
/// color; and in a week's or a day's hours, a tinted box with its color at
/// its left edge, its title, and its time when there's room. A selected
/// entry fills with its color, or the primary color when it has none.
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
    let color = grid.colors[shown.entry].map(|color| crate::theme::plugin_color(ui.ctx(), color));
    let timed = shown.segment.is_some();
    let in_month = timed && grid.mode == CalendarMode::Month;
    let rounding = radius(0.4);
    let (fill, foreground, quiet) = if selected {
        let fill = color.unwrap_or(palette.primary);
        let text = if color.is_some() {
            Color32::WHITE
        } else {
            palette.primary_foreground
        };
        (fill, text, text)
    } else if in_month {
        let fill = if response.hovered() {
            palette.accent
        } else {
            Color32::TRANSPARENT
        };
        (fill, palette.foreground, palette.muted_foreground)
    } else {
        // An opaque tint, so the hour lines under the entry don't show
        // through it.
        let tint = if response.hovered() { 0.35 } else { 0.22 };
        let fill = color.map_or(
            if response.hovered() {
                palette.accent
            } else {
                palette.secondary
            },
            |color| palette.background.lerp_to_gamma(color, tint),
        );
        (fill, palette.foreground, palette.muted_foreground)
    };
    painter.rect_filled(rect, rounding, fill);
    let mut left = rect.left() + 6.0;
    if in_month {
        let dot = if selected {
            Color32::WHITE
        } else {
            color.unwrap_or(palette.muted_foreground)
        };
        painter.circle_filled(pos2(rect.left() + 7.0, rect.center().y), 3.5, dot);
        left = rect.left() + 14.0;
    } else if timed && !selected {
        let edge = Rect::from_min_max(rect.min, pos2(rect.left() + 3.0, rect.bottom()));
        painter.rect_filled(
            edge,
            egui::CornerRadius {
                nw: 4,
                sw: 4,
                ne: 0,
                se: 0,
            },
            color.unwrap_or(palette.border),
        );
        left = rect.left() + 7.0;
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect.shrink(1.0),
            rounding,
            Stroke::new(1.5, palette.ring),
            StrokeKind::Inside,
        );
    }
    let width = rect.right() - left - 4.0;
    let title = text(ui, grid.entries[shown.entry].title, width, SUPPORTING_SIZE);
    let title_height = title.size().y;
    if in_month || !timed {
        painter.galley(
            Align2::LEFT_CENTER
                .anchor_size(pos2(left, rect.center().y), title.size())
                .min,
            title,
            foreground,
        );
        return;
    }
    painter.galley(pos2(left, rect.top() + 3.0), title, foreground);
    if rect.height() >= title_height * 2.0 + 8.0 {
        let times = shown.label.rsplit_once(", ").map_or("", |(_, times)| times);
        let times = text(ui, times, width, SUPPORTING_SIZE);
        painter.galley(pos2(left, rect.top() + 4.0 + title_height), times, quiet);
    }
}
