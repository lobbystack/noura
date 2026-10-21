//! The file tree widget. It draws only the rows in view and sends keys,
//! typing, clicks, drags and menu choices to `app-core`, which owns the
//! tree's state and carries out every change.

use std::time::Instant;

use app_core::{App, DropSpot, EntryKind, TreeAction, TreeKey, TreeRow};
use egui::{
    Align, Align2, Event, EventFilter, FontId, Id, Key, LayerId, Layout, Modifiers, Order, Popup,
    PopupKind, Pos2, Rect, Response, ScrollArea, Sense, Shape, Stroke, StrokeKind, TextEdit,
    TextWrapMode, Ui, UiBuilder, WidgetInfo, WidgetText,
    accesskit::{Action, Role},
    containers::menu::menu_style,
    pos2,
    text::{CCursor, CCursorRange},
    text_edit::TextEditState,
    vec2,
};

use crate::theme::{BODY_SIZE, Palette, SUPPORTING_SIZE, radius};

/// The height of one row.
pub const ROW_HEIGHT: f32 = 28.0;
const INDENT: f32 = 12.0;

/// What the tree remembers between frames.
#[derive(Default)]
pub(crate) struct TreeView {
    /// The workspace this state belongs to.
    workspace: Option<String>,
    offset: f32,
    /// The rows that existed last frame, to find the row that has keyboard
    /// focus: the rows in view and the tree's current row.
    drawn: Vec<(Id, String)>,
    /// The open row menu.
    menu: Option<RowMenu>,
    /// The text of the row being renamed in place.
    rename: Option<RenameField>,
    /// Give the current row keyboard focus again, once a menu or a rename
    /// field closes.
    refocus: bool,
}

impl TreeView {
    /// Give the tree's current row keyboard focus on the next frame.
    pub(crate) fn refocus(&mut self) {
        self.refocus = true;
    }
}

#[derive(Clone)]
struct RowMenu {
    relative_path: String,
    /// Where a right-click opened the menu. A menu opened from the keyboard
    /// opens below its row.
    at: Option<Pos2>,
    /// Give the first item keyboard focus, as a menu opened from the
    /// keyboard does.
    focus_first: bool,
}

struct RenameField {
    relative_path: String,
    text: String,
    /// The field needs keyboard focus: just opened, or a name was refused.
    focus: bool,
    /// Select the name, without a file's extension, so typing replaces it.
    select: bool,
}

/// A choice from a row's menu.
enum Command {
    NewNote(String),
    NewFolder(String),
    Rename(String),
    Duplicate(String),
    Trash(String),
    Reveal(String),
    CopyPath(String),
    Terminal(String),
}

/// What happened to the rename field this frame.
enum RenameOutcome {
    /// Enter: rename, and keep the field open if Noura refuses the name.
    Commit(String),
    /// Focus left the field: rename, or keep the old name and say why.
    Finish(String),
    Cancel,
}

fn row_id(relative_path: &str) -> Id {
    Id::unique(("file-tree-row", relative_path))
}

fn rename_id(relative_path: &str) -> Id {
    Id::unique(("file-tree-rename", relative_path))
}

/// Keys the tree takes while one of its rows has focus.
const KEY_FILTER: EventFilter = EventFilter {
    tab: false,
    horizontal_arrows: true,
    vertical_arrows: true,
    escape: false,
};

enum Input {
    Key(TreeKey),
    Text(String),
}

/// The tree key for a key press, if the tree uses it.
fn tree_key(key: Key, modifiers: Modifiers) -> Option<TreeKey> {
    if modifiers.matches_exact(Modifiers::SHIFT) {
        return (key == Key::F10).then_some(TreeKey::Menu);
    }
    // Cmd-Backspace on macOS, Ctrl-Backspace on Windows and Linux.
    if modifiers.matches_exact(Modifiers::COMMAND) {
        return (key == Key::Backspace).then_some(TreeKey::Trash);
    }
    if modifiers.any() {
        return None;
    }
    Some(match key {
        Key::ArrowUp => TreeKey::Up,
        Key::ArrowDown => TreeKey::Down,
        Key::Home => TreeKey::Home,
        Key::End => TreeKey::End,
        Key::ArrowRight => TreeKey::Right,
        Key::ArrowLeft => TreeKey::Left,
        Key::Enter => TreeKey::Enter,
        Key::Space => TreeKey::Activate,
        Key::F2 => TreeKey::Rename,
        Key::Delete => TreeKey::Trash,
        Key::Menu => TreeKey::Menu,
        _ => return None,
    })
}

/// Take the tree's keys and typing out of this frame's input.
fn take_input(ui: &Ui) -> Vec<Input> {
    ui.input_mut(|input| {
        let mut taken = Vec::new();
        input.events.retain(|event| {
            let found = match event {
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => tree_key(*key, *modifiers).map(Input::Key),
                Event::Text(text) if !text.trim().is_empty() => Some(Input::Text(text.clone())),
                _ => None,
            };
            match found {
                Some(found) => {
                    taken.push(found);
                    false
                }
                None => true,
            }
        });
        taken
    })
}

/// The header above the tree: New note, New folder and Collapse all.
fn header(ui: &mut Ui, app: &mut App) {
    let palette = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Files")
                .size(SUPPORTING_SIZE)
                .color(palette.muted_foreground),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if icon_button(ui, "Collapse all", Icon::CollapseAll).clicked() {
                app.collapse_all();
            }
            if icon_button(ui, "New folder", Icon::NewFolder).clicked() {
                app.new_folder(None);
            }
            if icon_button(ui, "New note", Icon::NewNote).clicked() {
                app.new_note(None);
            }
        });
    });
}

#[derive(Clone, Copy)]
enum Icon {
    NewNote,
    NewFolder,
    CollapseAll,
}

/// A small button that shows an icon and names itself for screen readers
/// and in its tooltip.
fn icon_button(ui: &mut Ui, label: &str, icon: Icon) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), label));
    let palette = Palette::of(ui.ctx());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() || response.has_focus() {
            painter.rect_filled(rect, radius(0.6), palette.sidebar_accent);
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect,
                radius(0.6),
                Stroke::new(1.0, palette.ring),
                StrokeKind::Inside,
            );
        }
        let stroke = Stroke::new(1.2, palette.muted_foreground);
        let c = rect.center();
        match icon {
            Icon::NewNote => {
                let page = Rect::from_center_size(c, vec2(10.0, 13.0));
                painter.rect_stroke(page, 1.0, stroke, StrokeKind::Middle);
                painter.line_segment([c + vec2(-2.5, 0.0), c + vec2(2.5, 0.0)], stroke);
                painter.line_segment([c + vec2(0.0, -2.5), c + vec2(0.0, 2.5)], stroke);
            }
            Icon::NewFolder => {
                let body = Rect::from_center_size(c + vec2(0.0, 1.0), vec2(14.0, 10.0));
                painter.rect_stroke(body, 1.0, stroke, StrokeKind::Middle);
                painter.line_segment(
                    [
                        body.left_top() + vec2(1.0, -2.0),
                        body.left_top() + vec2(5.0, -2.0),
                    ],
                    stroke,
                );
                let plus = body.center();
                painter.line_segment([plus + vec2(-2.5, 0.0), plus + vec2(2.5, 0.0)], stroke);
                painter.line_segment([plus + vec2(0.0, -2.5), plus + vec2(0.0, 2.5)], stroke);
            }
            Icon::CollapseAll => {
                painter.line_segment([c + vec2(-6.0, 0.0), c + vec2(6.0, 0.0)], stroke);
                for direction in [-1.0, 1.0] {
                    let tip = c + vec2(0.0, 2.0 * direction);
                    let back = 4.0 * direction;
                    painter.line_segment([tip + vec2(-3.5, back), tip], stroke);
                    painter.line_segment([tip + vec2(3.5, back), tip], stroke);
                }
            }
        }
    }
    response.on_hover_text(label)
}

pub(crate) fn show(ui: &mut Ui, app: &mut App, view: &mut TreeView) {
    // Another workspace starts at the top, with nothing focused.
    let workspace = app.workspace().map(|workspace| workspace.id().to_owned());
    let switched = view.workspace != workspace;
    if switched {
        *view = TreeView {
            workspace,
            ..TreeView::default()
        };
    }
    header(ui, app);
    follow_rename(app, view);

    // The tree has the keyboard when one of last frame's rows has focus, or
    // when a screen reader asks to focus one. A screen reader can focus any
    // row, as it can a row with tabindex -1 in the legacy tree. An open
    // menu keeps the keyboard.
    let asked = ui.input(|input| {
        view.drawn
            .iter()
            .find(|(id, _)| input.has_accesskit_action_request(*id, Action::Focus))
            .map(|(_, path)| path.clone())
    });
    let focused_path = asked.or_else(|| {
        ui.memory(|memory| memory.focused()).and_then(|focused| {
            view.drawn
                .iter()
                .find(|(id, _)| *id == focused)
                .map(|(_, path)| path.clone())
        })
    });
    let has_keyboard = focused_path.is_some() && view.menu.is_none();
    let mut reveal = false;
    if has_keyboard && let Some(path) = focused_path {
        if let Some(tree) = app.tree_mut()
            && let Some(row) = tree.find_row(&path)
        {
            tree.set_focus(row);
        }
        for input in take_input(ui) {
            reveal = true;
            match input {
                Input::Key(key) => {
                    if let Some(TreeAction::Menu { relative_path }) = app.press_tree_key(key) {
                        view.menu = Some(RowMenu {
                            relative_path,
                            at: None,
                            focus_first: true,
                        });
                    }
                }
                Input::Text(text) => {
                    if let Some(tree) = app.tree_mut() {
                        tree.type_ahead(&text, Instant::now());
                    }
                }
            }
        }
        follow_rename(app, view);
    }
    if let Some(tree) = app.tree_mut()
        && tree.dragging().is_some()
        && ui.input(|input| input.key_pressed(Key::Escape))
    {
        tree.cancel_drag();
    }

    let open_path = app.document().map(|file| file.relative_path.clone());
    let can_open_terminal = app.can_open_terminal();
    let reveal_label = app.platform().reveal_label();
    let problem = app.renaming().and_then(|rename| rename.problem.clone());
    let Some(tree) = app.tree() else {
        return;
    };
    let rows = tree.rows();
    let focused = tree.focused();
    let dragging = tree.dragging().map(str::to_owned);
    let drop_target = tree.drop_target().map(str::to_owned);
    // Like the legacy tree's roving tabindex, only the current row, or the
    // first row when none is current, takes Tab focus. Tab and Shift+Tab
    // leave the tree instead of stepping through its rows.
    let tab_stop = focused.or((!rows.is_empty()).then_some(0));
    let palette = Palette::of(ui.ctx());

    let mut scroll = ScrollArea::vertical()
        .id_salt("file-tree")
        .auto_shrink(false);
    if switched {
        scroll = scroll.vertical_scroll_offset(0.0);
    } else if reveal && let Some(row) = focused {
        let viewport = ui.available_height();
        let top = row as f32 * ROW_HEIGHT;
        let offset = if top < view.offset {
            top
        } else if top + ROW_HEIGHT > view.offset + viewport {
            top + ROW_HEIGHT - viewport
        } else {
            view.offset
        };
        scroll = scroll.vertical_scroll_offset(offset);
    }

    let mut clicked = None;
    let mut gained = None;
    let mut secondary = None;
    let mut drag_started = None;
    let mut command = None;
    let mut menu_closed = false;
    let mut renamed = None;
    let mut rects: Vec<(usize, Rect)> = Vec::new();
    let mut drawn = Vec::new();
    let menu = view.menu.clone();
    let rename = &mut view.rename;
    let mut row_ui = |ui: &mut Ui, index: usize, rect: Rect| {
        let row = &rows[index];
        let id = row_id(&row.relative_path);
        let state = RowState {
            selected: open_path.as_deref() == Some(row.relative_path.as_str()),
            tab_stop: Some(index) == tab_stop,
            renaming: rename
                .as_ref()
                .is_some_and(|field| field.relative_path == row.relative_path),
            drop_target: drop_target.as_deref() == Some(row.relative_path.as_str()),
            dragged: dragging.as_deref() == Some(row.relative_path.as_str()),
        };
        let response = draw_row(ui, rect, id, row, &state, &palette);
        if response.clicked() {
            clicked = Some(index);
        }
        if response.secondary_clicked() {
            secondary = Some((index, ui.input(|input| input.pointer.interact_pos())));
        }
        if response.drag_started() {
            drag_started = Some(index);
        }
        if response.gained_focus() {
            gained = Some(index);
            response.scroll_to_me(None);
        }
        if has_keyboard && Some(index) == focused && !response.has_focus() {
            response.request_focus();
        }
        if response.has_focus() {
            ui.memory_mut(|memory| memory.set_focus_lock_filter(id, KEY_FILTER));
        }
        if state.renaming
            && let Some(field) = rename.as_mut()
            && let Some(outcome) = rename_field(ui, rect, row, field, problem.as_deref())
        {
            renamed = Some(outcome);
        }
        if let Some(menu) = menu
            .as_ref()
            .filter(|menu| menu.relative_path == row.relative_path)
        {
            let shown = row_menu(ui, &response, row, menu, can_open_terminal, reveal_label);
            match shown {
                MenuOutcome::Open => {}
                MenuOutcome::Closed => menu_closed = true,
                MenuOutcome::Chose(chosen) => command = Some(chosen),
            }
        }
        rects.push((index, rect));
        drawn.push((id, row.relative_path.clone()));
    };
    let builder = UiBuilder::new()
        .accessibility_role(Role::Tree)
        .accessibility_label("Files");
    let output = ui
        .scope_builder(builder, |ui| {
            if drop_target.as_deref() == Some("") {
                ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                    node.set_description(DROP_TARGET);
                });
            }
            ui.spacing_mut().item_spacing.y = 0.0;
            scroll.show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
                // The current row exists even while it is scrolled out of
                // view, so it keeps keyboard focus and its place in the tab
                // order.
                let shown = ui.max_rect();
                let top = shown.top() - range.start as f32 * ROW_HEIGHT;
                let away = |index: usize| {
                    Rect::from_min_size(
                        pos2(shown.left(), top + index as f32 * ROW_HEIGHT),
                        vec2(shown.width(), ROW_HEIGHT),
                    )
                };
                if let Some(row) = tab_stop.filter(|row| *row < range.start) {
                    row_ui(ui, row, away(row));
                }
                for index in range.clone() {
                    let (_, rect) = ui.allocate_space(vec2(ui.available_width(), ROW_HEIGHT));
                    row_ui(ui, index, rect);
                }
                if let Some(row) = tab_stop.filter(|row| *row >= range.end) {
                    row_ui(ui, row, away(row));
                }
            })
        })
        .inner;
    view.offset = output.state.clamped_offset().y;
    view.drawn = drawn;
    let area = output.inner_rect;
    if drop_target.as_deref() == Some("") {
        ui.painter().rect_stroke(
            area,
            radius(0.6),
            Stroke::new(1.5, palette.ring),
            StrokeKind::Inside,
        );
    }

    if let Some(chosen) = command {
        view.menu = None;
        run(app, chosen);
    } else if menu_closed {
        view.menu = None;
        view.refocus = true;
    }
    if let Some(outcome) = renamed {
        finish_rename(app, view, outcome);
    }

    let Some(tree) = app.tree_mut() else {
        return;
    };
    if let Some(row) = gained {
        tree.set_focus(row);
    }
    if let Some((row, at)) = secondary
        && let Some(path) = tree.rows().get(row).map(|row| row.relative_path.clone())
    {
        tree.set_focus(row);
        ui.memory_mut(|memory| memory.request_focus(row_id(&path)));
        view.menu = Some(RowMenu {
            relative_path: path,
            at,
            focus_first: false,
        });
    }
    if let Some(row) = clicked {
        if let Some(path) = tree.rows().get(row).map(|row| row.relative_path.clone()) {
            ui.memory_mut(|memory| memory.request_focus(row_id(&path)));
        }
        if let Some(action) = tree.activate(row) {
            app.apply(action);
        }
    }
    drag(ui, app, drag_started, &rects, area);
    files(ui, app, &rects, area);
    if view.refocus {
        view.refocus = false;
        if let Some(tree) = app.tree()
            && let Some(row) = tree.focused()
        {
            let path = tree.rows()[row].relative_path.clone();
            ui.memory_mut(|memory| memory.request_focus(row_id(&path)));
        }
    }
}

/// Keep the rename field in step with the row `app-core` is renaming.
fn follow_rename(app: &App, view: &mut TreeView) {
    match app.renaming() {
        None => view.rename = None,
        Some(rename) => {
            let current = view
                .rename
                .as_ref()
                .is_some_and(|field| field.relative_path == rename.relative_path);
            if !current {
                view.rename = Some(RenameField {
                    relative_path: rename.relative_path.clone(),
                    text: rename.name.clone(),
                    focus: true,
                    select: true,
                });
                view.menu = None;
            }
        }
    }
}

fn finish_rename(app: &mut App, view: &mut TreeView, outcome: RenameOutcome) {
    match outcome {
        RenameOutcome::Commit(name) => {
            if app.commit_rename(&name) {
                view.rename = None;
                view.refocus = true;
            } else if let Some(field) = &mut view.rename {
                field.focus = true;
            }
        }
        RenameOutcome::Finish(name) => {
            app.finish_rename(&name);
            view.rename = None;
        }
        RenameOutcome::Cancel => {
            app.cancel_rename();
            view.rename = None;
            view.refocus = true;
        }
    }
}

/// Read by screen readers after a folder's name, and by tests, while a
/// drop would land in that folder.
const DROP_TARGET: &str = "drop target";

/// Where `pointer` is: over a row, over the blank part of the tree below
/// the rows, or away from the tree.
fn spot_at(pointer: Pos2, rects: &[(usize, Rect)], area: Rect) -> DropSpot {
    if !area.contains(pointer) {
        return DropSpot::Away;
    }
    rects
        .iter()
        .find(|(_, rect)| rect.contains(pointer))
        .map_or(DropSpot::Blank, |(row, _)| DropSpot::Row(*row))
}

/// Files dragged in from the file manager: the folder under the pointer
/// lights up, and dropped files are copied into it. When the window can't
/// tell where the pointer is, as on Linux, they go to the top of the
/// workspace.
fn files(ui: &Ui, app: &mut App, rects: &[(usize, Rect)], area: Rect) {
    let (hovering, dropped, pointer) = ui.input(|input| {
        let dropped: Vec<_> = input
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_owned())
            .collect();
        (
            !input.raw.hovered_files.is_empty(),
            dropped,
            input.pointer.latest_pos(),
        )
    });
    let Some(tree) = app.tree_mut() else {
        return;
    };
    if !hovering && dropped.is_empty() {
        tree.files_left();
        return;
    }
    let spot = pointer.map_or(DropSpot::Blank, |pointer| spot_at(pointer, rects, area));
    let now = Instant::now();
    tree.files_over(spot, now);
    if !dropped.is_empty() {
        app.drop_files(&dropped);
    } else if let Some(open_at) = tree.spring_open_at() {
        ui.ctx()
            .request_repaint_after(open_at.saturating_duration_since(now));
    }
}

/// Start, follow and drop a row drag. Over a folder the drop goes into it,
/// over a file into the file's folder, and over the blank part of the tree
/// to the top of the workspace. Past the window's edge `app-core` turns the
/// row into a native drag where the platform has one.
fn drag(ui: &mut Ui, app: &mut App, started: Option<usize>, rects: &[(usize, Rect)], area: Rect) {
    let Some(tree) = app.tree_mut() else {
        return;
    };
    if let Some(row) = started {
        tree.start_drag(row);
    }
    let Some(dragged) = tree.dragging().map(str::to_owned) else {
        return;
    };
    let (pointer, latest, released, window) = ui.input(|input| {
        (
            input.pointer.interact_pos(),
            input.pointer.latest_pos(),
            input.pointer.any_released(),
            input.viewport_rect(),
        )
    });
    let spot = pointer.map_or(DropSpot::Away, |pointer| spot_at(pointer, rects, area));
    let now = Instant::now();
    tree.drag_over(spot, now);
    if released {
        if let Some(action) = tree.finish_drag() {
            app.apply(action);
        }
        return;
    }
    // Past the window's edge the row becomes a native drag, where the
    // platform has one, so it can land in the file manager.
    if latest.is_none_or(|pointer| !window.shrink(1.0).contains(pointer)) {
        app.drag_row_out();
    }
    let Some(tree) = app.tree() else {
        return;
    };
    if tree.dragging().is_none() {
        return;
    }
    if let Some(open_at) = tree.spring_open_at() {
        ui.ctx()
            .request_repaint_after(open_at.saturating_duration_since(now));
    }
    // The dragged row's name follows the pointer.
    if let Some(pointer) = pointer {
        let name = tree
            .find_row(&dragged)
            .map(|row| tree.rows()[row].name.clone())
            .unwrap_or_default();
        let palette = Palette::of(ui.ctx());
        let painter = ui
            .ctx()
            .layer_painter(LayerId::new(Order::Tooltip, Id::unique("file-tree-drag")));
        let galley = WidgetText::from(name.as_str()).into_galley(
            ui,
            Some(TextWrapMode::Truncate),
            180.0,
            FontId::proportional(SUPPORTING_SIZE),
        );
        let label =
            Rect::from_min_size(pointer + vec2(12.0, 8.0), galley.size()).expand2(vec2(8.0, 4.0));
        painter.rect_filled(label, radius(0.8), palette.popover);
        painter.rect_stroke(
            label,
            radius(0.8),
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );
        painter.galley(label.min + vec2(8.0, 4.0), galley, palette.foreground);
    }
}

fn run(app: &mut App, command: Command) {
    match command {
        Command::NewNote(folder) => app.new_note(Some(&folder)),
        Command::NewFolder(folder) => app.new_folder(Some(&folder)),
        Command::Rename(path) => app.start_rename(&path),
        Command::Duplicate(path) => app.duplicate(&path),
        Command::Trash(path) => app.request_trash(&path),
        Command::Reveal(path) => app.reveal(&path),
        Command::CopyPath(path) => app.copy_path(&path),
        Command::Terminal(path) => app.open_terminal(&path),
    }
}

enum MenuOutcome {
    Open,
    Closed,
    Chose(Command),
}

/// The row's menu, below the row or where a right-click opened it.
fn row_menu(
    ui: &mut Ui,
    response: &Response,
    row: &TreeRow,
    menu: &RowMenu,
    can_open_terminal: bool,
    reveal_label: &str,
) -> MenuOutcome {
    let mut open = true;
    let mut chosen = None;
    let at = menu
        .at
        .unwrap_or_else(|| response.rect.left_bottom() + vec2(24.0, 0.0));
    let path = row.relative_path.clone();
    Popup::from_response(response)
        .id(response.id.with("menu"))
        .kind(PopupKind::Menu)
        .layout(Layout::top_down_justified(Align::Min))
        .style(menu_style)
        .at_position(at)
        .open_bool(&mut open)
        .accessible_name(format!("Actions for {}", row.name))
        .show(|ui| {
            ui.set_min_width(180.0);
            let mut first = true;
            let mut item = |ui: &mut Ui, label: &str, shortcut: Option<&str>| {
                let mut button = egui::Button::new(label);
                if let Some(shortcut) = shortcut {
                    button = button.shortcut_text(shortcut);
                }
                let response = ui.add(button).accessible_name(label);
                if let Some(shortcut) = shortcut {
                    let shortcut = shortcut.to_owned();
                    ui.ctx().accesskit_node_builder(response.id, |node| {
                        node.set_keyboard_shortcut(shortcut);
                    });
                }
                if first && menu.focus_first && !response.has_focus() {
                    response.request_focus();
                }
                first = false;
                response.clicked()
            };
            if row.kind == EntryKind::Folder {
                if item(ui, "New note", None) {
                    chosen = Some(Command::NewNote(path.clone()));
                }
                if item(ui, "New folder", None) {
                    chosen = Some(Command::NewFolder(path.clone()));
                }
                ui.separator();
            }
            if item(ui, "Rename\u{2026}", Some("F2")) {
                chosen = Some(Command::Rename(path.clone()));
            }
            if row.kind == EntryKind::File && item(ui, "Duplicate", None) {
                chosen = Some(Command::Duplicate(path.clone()));
            }
            ui.separator();
            if item(ui, reveal_label, None) {
                chosen = Some(Command::Reveal(path.clone()));
            }
            if item(ui, "Copy path", None) {
                chosen = Some(Command::CopyPath(path.clone()));
            }
            if can_open_terminal && item(ui, "Open in Terminal", None) {
                chosen = Some(Command::Terminal(path.clone()));
            }
            ui.separator();
            let trash_key = if cfg!(target_os = "macos") {
                "Cmd+Backspace"
            } else {
                "Delete"
            };
            if item(ui, "Move to Trash", Some(trash_key)) {
                chosen = Some(Command::Trash(path.clone()));
            }
        });
    if let Some(chosen) = chosen {
        ui.close();
        return MenuOutcome::Chose(chosen);
    }
    if open {
        MenuOutcome::Open
    } else {
        MenuOutcome::Closed
    }
}

/// The text field that renames `row` in place, over the row's name.
fn rename_field(
    ui: &mut Ui,
    rect: Rect,
    row: &TreeRow,
    field: &mut RenameField,
    problem: Option<&str>,
) -> Option<RenameOutcome> {
    let id = rename_id(&row.relative_path);
    // The field keeps a selection only while it has focus, so select once
    // focus arrives.
    if field.select && ui.memory(|memory| memory.has_focus(id)) {
        field.select = false;
        // Select the name without a file's extension, as Finder does.
        let end = match (row.kind, field.text.rfind('.')) {
            (EntryKind::File, Some(dot)) if dot > 0 => dot,
            _ => field.text.len(),
        };
        let end = field.text[..end].chars().count();
        let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(end))));
        state.store(ui.ctx(), id);
    }
    let inner = rect.shrink2(vec2(4.0, 2.0));
    let left = inner.left() + 6.0 + row.depth as f32 * INDENT + 12.0;
    let text_rect =
        Rect::from_min_max(pos2(left, inner.top()), pos2(inner.right(), inner.bottom()));
    let response = ui
        .put(
            text_rect,
            TextEdit::singleline(&mut field.text)
                .id(id)
                .font(FontId::proportional(BODY_SIZE))
                .desired_width(text_rect.width()),
        )
        .accessible_name(format!("New name for {}", row.name));
    if let Some(problem) = problem {
        let problem = problem.to_owned();
        ui.ctx()
            .accesskit_node_builder(response.id, |node| node.set_description(problem.clone()));
        Popup::from_response(&response)
            .id(id.with("problem"))
            .open(true)
            .show(|ui| {
                ui.set_max_width(240.0);
                let palette = Palette::of(ui.ctx());
                ui.label(egui::RichText::new(problem).color(palette.destructive));
            });
    }
    if field.focus {
        field.focus = false;
        response.request_focus();
    }
    if !response.lost_focus() {
        return None;
    }
    let (escape, enter) = ui.input(|input| {
        (
            input.key_pressed(Key::Escape),
            input.key_pressed(Key::Enter),
        )
    });
    Some(if escape {
        RenameOutcome::Cancel
    } else if enter {
        RenameOutcome::Commit(field.text.clone())
    } else {
        RenameOutcome::Finish(field.text.clone())
    })
}

/// How a row looks this frame.
struct RowState {
    /// The row's file is open in the document area.
    selected: bool,
    /// Only the tab stop takes Tab focus; every row takes clicks.
    tab_stop: bool,
    renaming: bool,
    /// A drag would drop into this folder.
    drop_target: bool,
    dragged: bool,
}

/// One row at `rect`.
fn draw_row(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    row: &TreeRow,
    state: &RowState,
    palette: &Palette,
) -> Response {
    let sense = if state.tab_stop {
        Sense::click_and_drag()
    } else {
        Sense::CLICK | Sense::DRAG
    };
    let mut response = ui.interact(rect, id, sense);
    response.widget_info(|| WidgetInfo::labeled(Role::TreeItem, true, &row.name));
    let description = row_description(row, state.drop_target);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.add_action(Action::Focus);
        node.set_level(row.depth);
        if let Some(expanded) = row.expanded {
            node.set_expanded(expanded);
        }
        if state.selected {
            node.set_selected(true);
        }
        if let Some(description) = description {
            node.set_description(description);
        }
    });
    if row.not_downloaded {
        response = response.on_hover_text("Not downloaded");
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let inner = rect.shrink2(vec2(4.0, 1.0));
    if state.selected || response.hovered() || state.drop_target {
        painter.rect_filled(inner, radius(0.6), palette.sidebar_accent);
    }
    if response.has_focus() || state.drop_target {
        painter.rect_stroke(
            inner,
            radius(0.6),
            Stroke::new(1.0, palette.ring),
            StrokeKind::Inside,
        );
    }
    let left = inner.left() + 6.0 + row.depth as f32 * INDENT;
    let center_y = rect.center().y;
    if let Some(expanded) = row.expanded {
        painter.add(chevron(pos2(left + 5.0, center_y), expanded, palette));
    }
    if state.renaming {
        return response;
    }
    let text_left = left + 16.0;
    let width = (inner.right() - text_left - 4.0).max(0.0);
    let galley = WidgetText::from(row.name.as_str()).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        width,
        FontId::proportional(BODY_SIZE),
    );
    let text_rect = Align2::LEFT_CENTER.anchor_size(pos2(text_left, center_y), galley.size());
    let color = if row.not_downloaded || state.dragged {
        palette.muted_foreground
    } else {
        palette.foreground
    };
    painter.galley(text_rect.min, galley, color);
    response
}

/// What a screen reader hears after the row's name. A file cloud storage
/// hasn't downloaded says so. On macOS a folder also says whether it is
/// open: accesskit_macos 0.27 doesn't pass a row's expanded state or level
/// to VoiceOver, while the Windows and Linux adapters do.
fn row_description(row: &TreeRow, drop_target: bool) -> Option<String> {
    let mut parts = Vec::new();
    if cfg!(target_os = "macos")
        && let Some(expanded) = row.expanded
    {
        parts.push(if expanded { "expanded" } else { "collapsed" });
    }
    if row.not_downloaded {
        parts.push("not downloaded");
    }
    if drop_target {
        parts.push(DROP_TARGET);
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// A small triangle: pointing right for a closed folder, down for an open one.
fn chevron(center: Pos2, expanded: bool, palette: &Palette) -> Shape {
    let size = 3.5;
    let points = if expanded {
        vec![
            center + vec2(-size, -size * 0.5),
            center + vec2(size, -size * 0.5),
            center + vec2(0.0, size * 0.75),
        ]
    } else {
        vec![
            center + vec2(-size * 0.5, -size),
            center + vec2(-size * 0.5, size),
            center + vec2(size * 0.75, 0.0),
        ]
    };
    Shape::convex_polygon(points, palette.muted_foreground, Stroke::NONE)
}
