//! The file tree widget. It draws only the rows in view and sends keys,
//! typing and clicks to `app-core`'s tree, which owns the tree's state.

use std::time::Instant;

use app_core::{App, EntryKind, OpenAs, TreeKey, TreeRow};
use egui::{
    Align2, Event, EventFilter, FontId, Id, Key, Pos2, Rect, Response, ScrollArea, Sense, Shape,
    Stroke, StrokeKind, TextWrapMode, Ui, UiBuilder, WidgetInfo, WidgetText,
    accesskit::{Action, Role},
    pos2, vec2,
};

use crate::theme::{BODY_SIZE, Palette, radius};

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
}

fn row_id(relative_path: &str) -> Id {
    Id::unique(("file-tree-row", relative_path))
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
                } if !modifiers.any() => match key {
                    Key::ArrowUp => Some(Input::Key(TreeKey::Up)),
                    Key::ArrowDown => Some(Input::Key(TreeKey::Down)),
                    Key::Home => Some(Input::Key(TreeKey::Home)),
                    Key::End => Some(Input::Key(TreeKey::End)),
                    Key::ArrowRight => Some(Input::Key(TreeKey::Right)),
                    Key::ArrowLeft => Some(Input::Key(TreeKey::Left)),
                    Key::Enter | Key::Space => Some(Input::Key(TreeKey::Activate)),
                    _ => None,
                },
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
    // The tree has the keyboard when one of last frame's rows has focus, or
    // when a screen reader asks to focus one. A screen reader can focus any
    // row, as it can a row with tabindex -1 in the legacy tree.
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
    let has_keyboard = focused_path.is_some();
    let mut reveal = false;
    if let Some(path) = focused_path {
        if let Some(tree) = app.tree_mut()
            && let Some(row) = tree.find_row(&path)
        {
            tree.set_focus(row);
        }
        for input in take_input(ui) {
            reveal = true;
            match input {
                Input::Key(key) => app.press_tree_key(key),
                Input::Text(text) => {
                    if let Some(tree) = app.tree_mut() {
                        tree.type_ahead(&text, Instant::now());
                    }
                }
            }
        }
    }

    let open_path = app.document().map(|file| file.relative_path.clone());
    let Some(tree) = app.tree() else {
        return;
    };
    let rows = tree.rows();
    let focused = tree.focused();
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
    let mut kept = None;
    let mut gained = None;
    let mut drawn = Vec::new();
    let mut row_ui = |ui: &mut Ui, index: usize, rect: Rect| {
        let row = &rows[index];
        let id = row_id(&row.relative_path);
        let selected = open_path.as_deref() == Some(row.relative_path.as_str());
        let tab_stop = Some(index) == tab_stop;
        let response = draw_row(ui, rect, id, row, selected, tab_stop, &palette);
        if response.clicked() {
            // Cmd-click (Ctrl-click on Windows and Linux) opens a file in
            // a new tab, as Open in new tab does.
            let command = ui.input(|input| input.modifiers.command);
            clicked = Some((
                index,
                if command {
                    OpenAs::Kept
                } else {
                    OpenAs::Preview
                },
            ));
        }
        if row.kind == EntryKind::File {
            // A double-click keeps the preview tab the first click opened.
            if response.double_clicked() {
                kept = Some(row.relative_path.clone());
            }
            response.context_menu(|ui| {
                if ui.button("Open in new tab").clicked() {
                    kept = Some(row.relative_path.clone());
                    ui.close();
                }
            });
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
        drawn.push((id, row.relative_path.clone()));
    };
    let builder = UiBuilder::new()
        .accessibility_role(Role::Tree)
        .accessibility_label("Files");
    let output = ui
        .scope_builder(builder, |ui| {
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

    let Some(tree) = app.tree_mut() else {
        return;
    };
    if let Some(row) = gained {
        tree.set_focus(row);
    }
    if let Some((row, how)) = clicked {
        if let Some(path) = tree.rows().get(row).map(|row| row.relative_path.clone()) {
            ui.memory_mut(|memory| memory.request_focus(row_id(&path)));
        }
        app.activate_tree_row(row, how);
    }
    if let Some(path) = kept {
        app.open_file(&path, OpenAs::Kept);
    }
}

/// One row at `rect`. Only the tab stop takes Tab focus; every row takes
/// clicks.
fn draw_row(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    row: &TreeRow,
    selected: bool,
    tab_stop: bool,
    palette: &Palette,
) -> Response {
    let sense = if tab_stop {
        Sense::click()
    } else {
        Sense::CLICK
    };
    let mut response = ui.interact(rect, id, sense);
    response.widget_info(|| WidgetInfo::labeled(Role::TreeItem, true, &row.name));
    let description = row_description(row);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.add_action(Action::Focus);
        node.set_level(row.depth);
        if let Some(expanded) = row.expanded {
            node.set_expanded(expanded);
        }
        if selected {
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
    if selected || response.hovered() {
        painter.rect_filled(inner, radius(0.6), palette.sidebar_accent);
    }
    if response.has_focus() {
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
    let text_left = left + 16.0;
    let width = (inner.right() - text_left - 4.0).max(0.0);
    let galley = WidgetText::from(row.name.as_str()).into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        width,
        FontId::proportional(BODY_SIZE),
    );
    let text_rect = Align2::LEFT_CENTER.anchor_size(pos2(text_left, center_y), galley.size());
    let color = if row.not_downloaded {
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
fn row_description(row: &TreeRow) -> Option<String> {
    let mut parts = Vec::new();
    if cfg!(target_os = "macos")
        && let Some(expanded) = row.expanded
    {
        parts.push(if expanded { "expanded" } else { "collapsed" });
    }
    if row.not_downloaded {
        parts.push("not downloaded");
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// A small triangle: pointing right for a closed folder, down for an open one.
pub(crate) fn chevron(center: Pos2, expanded: bool, palette: &Palette) -> Shape {
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
