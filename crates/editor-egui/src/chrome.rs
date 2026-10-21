//! The editor's chrome: the formatting toolbar above the note, the small
//! toolbar over a selection, and the find bar. The commands they run live
//! in `editor-core`; this module draws them and turns clicks and keys into
//! events.
//!
//! The find bar adapts the find widget of Lockbook's editor
//! (`tab/markdown_editor/widget/find.rs`, Unlicense; see
//! `docs/provenance/lockbook-editor.md`), drawn with egui's own widgets.

use editor_core::commands::Format;
use editor_core::find::Find;
use editor_core::keymap::{self, Command};
use editor_core::offset_types::{Grapheme, RangeExt as _};
use egui::accesskit::Role;
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Align2, Button, Context, Frame, Key, Margin, ModifierNames, Modifiers, Order, Pos2, Rect,
    Response, RichText, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, vec2,
};

use crate::input::Event;
use crate::input::canonical::is_apple;
use crate::{Editor, ScrollTarget};

/// The text version and selection formats were read at.
type FormatsKey = (u64, (Grapheme, Grapheme));

/// What the editor draws around its text, and the state those parts keep.
#[derive(Default)]
pub(crate) struct Chrome {
    /// Draw the formatting toolbar and the selection toolbar.
    pub toolbars: bool,
    /// The find bar, while it's open.
    pub find: Option<FindBar>,
    /// The query find last closed with, for the next time it opens with
    /// nothing selected.
    last_query: String,
    /// An input method is composing text.
    composing: bool,
    /// The formats at the selection, for the toolbars' pressed states.
    formats: Vec<Format>,
    /// The text version and selection `formats` were read at.
    formats_read_at: Option<FormatsKey>,
    /// Where the selection toolbar drew last frame.
    selection_toolbar: Option<Rect>,
    /// Where the text drew, below the toolbar and the find bar.
    pub text_rect: Option<Rect>,
}

impl Chrome {
    pub(crate) fn new(toolbars: bool) -> Self {
        Self {
            toolbars,
            ..Self::default()
        }
    }
}

/// The find bar's state: find's matches and the two fields.
pub(crate) struct FindBar {
    pub find: Find,
    query: String,
    replacement: String,
    /// Put the keyboard in the query field with its text selected.
    focus_query: bool,
    /// Whether a field had the keyboard at the end of the last frame.
    /// egui takes the keyboard from a field on Escape before the bar
    /// draws, so Escape checks this.
    was_focused: bool,
}

/// What a button shows: a letter or two, or a painted icon. Icons are
/// painted so the toolbar needs no icon font; the app's icon set comes with
/// the theme work.
#[derive(Clone)]
enum Glyph {
    Text(RichText),
    Icon(Icon),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Icon {
    Link,
    BulletList,
    NumberedList,
    CheckList,
    Undo,
    Redo,
    Find,
    Up,
    Down,
    Close,
}

impl From<&str> for Glyph {
    fn from(text: &str) -> Self {
        Self::Text(RichText::new(text))
    }
}

impl From<RichText> for Glyph {
    fn from(text: RichText) -> Self {
        Self::Text(text)
    }
}

impl From<Icon> for Glyph {
    fn from(icon: Icon) -> Self {
        Self::Icon(icon)
    }
}

/// Paints `icon` in a 16-point square centered in `rect`.
fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, stroke: Stroke) {
    let c = rect.center();
    let at = |x: f32, y: f32| c + vec2(x, y);
    let fill = stroke.color;
    match icon {
        Icon::Link => {
            for x in [-3.0, 3.0] {
                let link = Rect::from_center_size(at(x, 0.0), vec2(9.0, 6.0));
                painter.rect_stroke(link, 3.0, stroke, StrokeKind::Middle);
            }
        }
        Icon::BulletList | Icon::NumberedList | Icon::CheckList => {
            for (row, y) in [-4.5, 0.0, 4.5].into_iter().enumerate() {
                match icon {
                    Icon::BulletList => {
                        painter.circle_filled(at(-5.0, y), 1.3, fill);
                    }
                    Icon::NumberedList => {
                        painter.text(
                            at(-5.0, y),
                            Align2::CENTER_CENTER,
                            (row + 1).to_string(),
                            egui::FontId::proportional(6.5),
                            fill,
                        );
                    }
                    _ => {
                        let box_rect = Rect::from_center_size(at(-5.0, y), vec2(3.4, 3.4));
                        painter.rect_stroke(box_rect, 0.5, stroke, StrokeKind::Middle);
                    }
                }
                painter.line_segment([at(-2.0, y), at(7.0, y)], stroke);
            }
        }
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Undo { 1.0 } else { -1.0 };
            let points: Vec<Pos2> = (0..=12)
                .map(|step| {
                    let angle = std::f32::consts::PI * (1.15 - 1.3 * step as f32 / 12.0);
                    at(flip * 5.0 * angle.cos(), 1.5 - 5.0 * angle.sin())
                })
                .collect();
            let tip = points[0];
            painter.add(egui::Shape::line(points, stroke));
            painter.line_segment([tip, tip + vec2(flip * 3.2, -0.6)], stroke);
            painter.line_segment([tip, tip + vec2(flip * 0.4, -3.2)], stroke);
        }
        Icon::Find => {
            painter.circle_stroke(at(-1.5, -1.5), 4.5, stroke);
            painter.line_segment([at(1.8, 1.8), at(5.5, 5.5)], stroke);
        }
        Icon::Up | Icon::Down => {
            let y = if icon == Icon::Up { 1.0 } else { -1.0 };
            painter.add(egui::Shape::line(
                vec![at(-4.0, 2.0 * y), at(0.0, -2.0 * y), at(4.0, 2.0 * y)],
                stroke,
            ));
        }
        Icon::Close => {
            painter.line_segment([at(-4.0, -4.0), at(4.0, 4.0)], stroke);
            painter.line_segment([at(-4.0, 4.0), at(4.0, -4.0)], stroke);
        }
    }
}

/// A frameless button showing `glyph`, framed while `selected` is true.
/// `selected` makes it a toggle for assistive technology.
fn glyph_button(ui: &mut Ui, glyph: &Glyph, selected: Option<bool>, enabled: bool) -> Response {
    let text = match glyph {
        Glyph::Text(text) => text.clone(),
        Glyph::Icon(_) => RichText::new(""),
    };
    let mut button = Button::new(text).min_size(vec2(28.0, 24.0));
    button = match selected {
        Some(selected) => button.selected(selected).frame_when_inactive(selected),
        None => button.frame_when_inactive(false),
    };
    let response = ui.add_enabled(enabled, button);
    if let Glyph::Icon(icon) = glyph {
        let visuals = ui
            .style()
            .interact_selectable(&response, selected.unwrap_or(false));
        let color = if enabled {
            visuals.fg_stroke.color
        } else {
            ui.visuals().weak_text_color()
        };
        paint_icon(ui.painter(), response.rect, *icon, Stroke::new(1.3, color));
    }
    response
}

/// One toolbar button: the command it runs, what it shows, and what
/// assistive technology and its tooltip call it.
struct Tool {
    command: Command,
    glyph: Glyph,
    name: &'static str,
}

fn tool(command: Command, glyph: impl Into<Glyph>, name: &'static str) -> Tool {
    Tool {
        command,
        glyph: glyph.into(),
        name,
    }
}

/// The inline formats, which both toolbars offer.
fn inline_tools() -> [Tool; 4] {
    [
        tool(
            Command::Format(Format::Bold),
            RichText::new("B").strong(),
            "Bold",
        ),
        tool(
            Command::Format(Format::Italic),
            RichText::new("I").italics(),
            "Italic",
        ),
        tool(
            Command::Format(Format::Underline),
            RichText::new("U").underline(),
            "Underline",
        ),
        tool(
            Command::Format(Format::Strikethrough),
            RichText::new("S").strikethrough(),
            "Strikethrough",
        ),
    ]
}

/// The block and link formats after the inline ones.
fn block_tools() -> [Tool; 6] {
    [
        tool(Command::Format(Format::Link), Icon::Link, "Link"),
        tool(
            Command::Format(Format::BlockQuote),
            RichText::new("“").size(18.0),
            "Quote",
        ),
        tool(Command::Format(Format::Code), "</>", "Inline code"),
        tool(
            Command::Format(Format::BulletList),
            Icon::BulletList,
            "Bulleted list",
        ),
        tool(
            Command::Format(Format::NumberedList),
            Icon::NumberedList,
            "Numbered list",
        ),
        tool(
            Command::Format(Format::CheckList),
            Icon::CheckList,
            "Checklist",
        ),
    ]
}

fn egui_key(key: keymap::Key) -> Key {
    use keymap::Key as K;
    match key {
        K::B => Key::B,
        K::C => Key::C,
        K::F => Key::F,
        K::I => Key::I,
        K::K => Key::K,
        K::Q => Key::Q,
        K::U => Key::U,
        K::X => Key::X,
        K::Y => Key::Y,
        K::Z => Key::Z,
        K::Enter => Key::Enter,
        K::Tab => Key::Tab,
        K::Num0 => Key::Num0,
        K::Num1 => Key::Num1,
        K::Num2 => Key::Num2,
        K::Num3 => Key::Num3,
        K::Num7 => Key::Num7,
        K::Num8 => Key::Num8,
        K::Num9 => Key::Num9,
    }
}

/// A shortcut as people read it: `⇧⌘X` on macOS and `Ctrl+Shift+X`
/// elsewhere.
pub(crate) fn shortcut_text(shortcut: keymap::Shortcut, apple: bool) -> String {
    let mut modifiers = Modifiers::NONE;
    if shortcut.command {
        modifiers |= Modifiers::COMMAND;
    }
    if shortcut.alt {
        modifiers |= Modifiers::ALT;
    }
    if shortcut.shift {
        modifiers |= Modifiers::SHIFT;
    }
    let names = if apple {
        ModifierNames::SYMBOLS
    } else {
        ModifierNames::NAMES
    };
    let mut text = names.format(&modifiers, apple);
    if !text.is_empty() {
        text += names.concat;
    }
    text + egui_key(shortcut.key).symbol_or_name()
}

/// A control's name with its first shortcut, as in `Bold (⌘B)`.
fn named_with_shortcut(name: &str, command: Command, apple: bool) -> String {
    match keymap::shortcuts_of(command, apple).first() {
        Some(shortcut) => format!("{name} ({})", shortcut_text(*shortcut, apple)),
        None => name.to_owned(),
    }
}

impl Editor {
    /// Whether the note takes formatting: an editable Markdown note.
    fn formattable(&self) -> bool {
        let renderer = &self.edit.renderer;
        self.chrome.toolbars && !renderer.readonly && !renderer.plaintext
    }

    /// The id of the find bar's query field.
    fn find_query_id(&self) -> egui::Id {
        self.id().with("find-query")
    }

    fn find_replace_id(&self) -> egui::Id {
        self.id().with("find-replace")
    }

    /// Whether one of the find bar's fields has the keyboard.
    pub fn find_focused(&self, ctx: &Context) -> bool {
        ctx.memory(|memory| {
            memory.has_focus(self.find_query_id()) || memory.has_focus(self.find_replace_id())
        })
    }

    /// Whether the find bar is open.
    pub fn find_open(&self) -> bool {
        self.chrome.find.is_some()
    }

    /// Opens find with the selected text as the query, or brings the
    /// keyboard back to it when it's already open.
    pub fn open_find(&mut self) {
        let doc = &self.edit.renderer.doc;
        let selection = doc.selection();
        let selected = doc.buffer[(selection.start(), selection.end())].to_owned();
        let selected = (!selected.is_empty() && !selected.contains('\n')).then_some(selected);
        let bar = self.chrome.find.get_or_insert_with(|| FindBar {
            find: Find::default(),
            query: String::new(),
            replacement: String::new(),
            focus_query: true,
            was_focused: false,
        });
        let query = match selected {
            Some(selected) => Some(selected),
            None if bar.query.is_empty() => Some(self.chrome.last_query.clone()),
            None => None,
        };
        if let Some(query) = query {
            bar.query = query;
            bar.find.set_query(doc, &bar.query);
        }
        bar.focus_query = true;
        if bar.find.current().is_some() {
            self.edit.pending_scroll = Some(ScrollTarget::FindMatch);
        }
    }

    /// Closes find and returns the keyboard to the text, with the current
    /// match selected.
    pub fn close_find(&mut self, ctx: &Context) {
        let Some(bar) = self.chrome.find.take() else {
            return;
        };
        if let Some(range) = bar.find.current() {
            self.push_event(Event::Select {
                region: range.into(),
            });
        }
        self.chrome.last_query = bar.query;
        self.set_find_match(None);
        self.focus(ctx);
        ctx.request_repaint();
    }

    /// Finds the matches again after the text changed.
    pub(crate) fn refresh_find(&mut self) {
        let Some(bar) = self.chrome.find.as_mut() else {
            return;
        };
        bar.find.refresh(&self.edit.renderer.doc);
        let current = bar.find.current();
        self.set_find_match(current);
    }

    /// Shows `range` as find's current match: its syntax reveals like the
    /// caret's.
    fn set_find_match(&mut self, range: Option<(Grapheme, Grapheme)>) {
        let renderer = &mut self.edit.renderer;
        if renderer.find_current_match != range {
            renderer.find_current_match = range;
            renderer.bump_reveal_seq();
        }
    }

    /// Notes what an input method is doing from this frame's events.
    pub(crate) fn track_composition(&mut self, ctx: &Context) {
        ctx.input(|input| {
            for event in &input.events {
                match event {
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                        self.chrome.composing = !text.is_empty();
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(_)) => {
                        self.chrome.composing = false;
                    }
                    _ => {}
                }
            }
        });
    }

    /// Mod-F opens find, unless another text field has the keyboard.
    pub(crate) fn handle_find_key(&mut self, ctx: &Context) {
        let elsewhere = ctx.text_edit_focused() && !self.find_focused(ctx);
        if elsewhere {
            return;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::F)) {
            self.open_find();
            ctx.request_repaint();
        }
    }

    /// Reads the formats at the selection for the toolbars, once per text
    /// version and selection.
    pub(crate) fn update_formats<'a>(&mut self, root: &'a comrak::nodes::AstNode<'a>) {
        if !self.formattable() {
            return;
        }
        let doc = &self.edit.renderer.doc;
        let key = Some((doc.text_seq, doc.selection()));
        if self.chrome.formats_read_at != key {
            self.chrome.formats = doc.formats_in(root);
            self.chrome.formats_read_at = key;
        }
    }

    fn pressed(&self, command: Command) -> bool {
        matches!(command, Command::Format(format) if self.chrome.formats.contains(&format))
    }

    /// Runs a command a toolbar button stands for and gives the keyboard
    /// back to the text.
    fn run_tool(&mut self, ctx: &Context, command: Command) {
        match command.event() {
            Some(event) => self.push_event(event),
            None => self.open_find(),
        }
        if command != Command::Find {
            self.focus(ctx);
        }
        ctx.request_repaint();
    }

    /// One toolbar button, labeled with its name and shortcut.
    fn tool_button(&self, ui: &mut Ui, tool: &Tool, toggles: bool, enabled: bool) -> bool {
        let apple = is_apple(ui.ctx());
        let name = named_with_shortcut(tool.name, tool.command, apple);
        let pressed = toggles.then(|| self.pressed(tool.command));
        glyph_button(ui, &tool.glyph, pressed, enabled)
            .on_hover_text(&name)
            .accessible_name(name)
            .clicked()
    }

    /// The formatting toolbar: paragraph style, the inline formats, links,
    /// quotes, code and lists, then undo, redo and find.
    pub(crate) fn show_toolbar(&mut self, ui: &mut Ui) {
        if !self.formattable() {
            return;
        }
        let ctx = ui.ctx().clone();
        let apple = is_apple(&ctx);
        let mut run = None;
        let builder = UiBuilder::new()
            .accessibility_role(Role::Toolbar)
            .accessibility_label("Formatting");
        ui.scope_builder(builder, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let style = self.edit.renderer.doc.paragraph_style();
                let combo = egui::ComboBox::from_id_salt(self.id().with("paragraph-style"))
                    .selected_text(Format::Heading(style).label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for level in 0..=3 {
                            let format = Format::Heading(level);
                            let name =
                                named_with_shortcut(format.label(), Command::Format(format), apple);
                            let item = ui
                                .selectable_label(style == level, format.label())
                                .on_hover_text(&name)
                                .accessible_name(name);
                            if item.clicked() {
                                run = Some(Command::Format(format));
                            }
                        }
                    });
                let combo = combo.response.accessible_name("Paragraph style");
                ui.ctx().accesskit_node_builder(combo.id, |node| {
                    node.set_value(Format::Heading(style).label());
                });
                ui.separator();
                for tool in inline_tools() {
                    if self.tool_button(ui, &tool, true, true) {
                        run = Some(tool.command);
                    }
                }
                ui.separator();
                for tool in block_tools() {
                    if self.tool_button(ui, &tool, true, true) {
                        run = Some(tool.command);
                    }
                }
                ui.separator();
                let buffer = &self.edit.renderer.buffer;
                let (can_undo, can_redo) = (buffer.can_undo(), buffer.can_redo());
                for (tool, enabled) in [
                    (tool(Command::Undo, Icon::Undo, "Undo"), can_undo),
                    (tool(Command::Redo, Icon::Redo, "Redo"), can_redo),
                    (tool(Command::Find, Icon::Find, "Find"), true),
                ] {
                    if self.tool_button(ui, &tool, false, enabled) {
                        run = Some(tool.command);
                    }
                }
            });
        });
        ui.separator();
        if let Some(command) = run {
            self.run_tool(&ctx, command);
        }
    }

    /// The find bar: the query with its match count, previous, next and
    /// close, then the replacement with Replace and Replace all. Enter
    /// steps to the next match and Shift-Enter to the previous one, and
    /// Escape closes the bar.
    pub(crate) fn show_find_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let query_id = self.find_query_id();
        let replace_id = self.find_replace_id();
        let Some(bar) = self.chrome.find.as_mut() else {
            return;
        };
        let doc = &self.edit.renderer.doc;
        bar.find.refresh(doc);

        let (query_focused, replace_focused) =
            ctx.memory(|memory| (memory.has_focus(query_id), memory.has_focus(replace_id)));
        let mut close = false;
        let mut step = None;
        let mut replace_one = false;
        let mut replace_all = false;
        let mut query_changed = false;
        if query_focused || replace_focused || bar.was_focused {
            close = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
        }
        if query_focused || replace_focused {
            // Taken before the fields see them: a one-line field gives up
            // the keyboard on Enter.
            let shift_enter =
                ctx.input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::Enter));
            let enter = ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter));
            if query_focused && shift_enter {
                step = Some(true);
            } else if query_focused && enter {
                step = Some(false);
            } else if replace_focused && (enter || shift_enter) {
                replace_one = true;
            }
        }

        let builder = UiBuilder::new()
            .accessibility_role(Role::Search)
            .accessibility_label("Find and replace");
        ui.scope_builder(builder, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(4, 2))
                .show(ui, |ui| {
                    let has_matches = !bar.find.matches().is_empty();
                    ui.horizontal(|ui| {
                        let field = ui
                            .add(
                                TextEdit::singleline(&mut bar.query)
                                    .id(query_id)
                                    .hint_text("Find in note")
                                    .desired_width(220.0),
                            )
                            .accessible_name("Find in note");
                        query_changed = field.changed();
                        if std::mem::take(&mut bar.focus_query) {
                            field.request_focus();
                            let mut state =
                                TextEditState::load(ui.ctx(), query_id).unwrap_or_default();
                            let end = CCursor::new(bar.query.chars().count());
                            state
                                .cursor
                                .set_char_range(Some(CCursorRange::two(CCursor::new(0), end)));
                            state.store(ui.ctx(), query_id);
                        }
                        ui.label(bar.find.count_label());
                        let previous = glyph_button(ui, &Icon::Up.into(), None, has_matches)
                            .on_hover_text("Previous match (Shift+Enter)")
                            .accessible_name("Previous match (Shift+Enter)");
                        if previous.clicked() {
                            step = Some(true);
                            // The keyboard stays in the query, so Enter and
                            // Escape keep working.
                            ui.memory_mut(|memory| memory.request_focus(query_id));
                        }
                        let next = glyph_button(ui, &Icon::Down.into(), None, has_matches)
                            .on_hover_text("Next match (Enter)")
                            .accessible_name("Next match (Enter)");
                        if next.clicked() {
                            step = Some(false);
                            ui.memory_mut(|memory| memory.request_focus(query_id));
                        }
                        let close_button = glyph_button(ui, &Icon::Close.into(), None, true)
                            .on_hover_text("Close (Esc)")
                            .accessible_name("Close (Esc)");
                        close |= close_button.clicked();
                    });
                    ui.horizontal(|ui| {
                        ui.add(
                            TextEdit::singleline(&mut bar.replacement)
                                .id(replace_id)
                                .hint_text("Replace with")
                                .desired_width(220.0),
                        )
                        .accessible_name("Replace with");
                        let one = ui
                            .add_enabled(has_matches, Button::new("Replace"))
                            .clicked();
                        let all = ui
                            .add_enabled(has_matches, Button::new("Replace all"))
                            .clicked();
                        if one || all {
                            ui.memory_mut(|memory| memory.request_focus(replace_id));
                        }
                        replace_one |= one;
                        replace_all |= all;
                    });
                });
        });
        ui.separator();
        bar.was_focused =
            ctx.memory(|memory| memory.has_focus(query_id) || memory.has_focus(replace_id));

        let mut scroll = false;
        if query_changed {
            bar.find.set_query(doc, &bar.query);
            scroll = bar.find.current().is_some();
        }
        if let Some(backwards) = step {
            scroll = bar.find.step(backwards);
        }
        let mut events = Vec::new();
        if replace_one && !self.edit.renderer.readonly {
            events = bar.find.replace_current(&bar.replacement);
        }
        if replace_all && !self.edit.renderer.readonly {
            events = bar.find.replace_all(&bar.replacement);
        }
        let current = bar.find.current();
        for event in events {
            self.push_event(event);
        }
        if scroll {
            self.edit.pending_scroll = Some(ScrollTarget::FindMatch);
        }
        if close {
            self.close_find(&ctx);
        } else {
            self.set_find_match(current);
        }
    }

    /// Paints find's matches among the fragments drawn this frame, the
    /// current one stronger.
    pub(crate) fn paint_find_matches(&self, ui: &mut Ui) {
        let Some(bar) = &self.chrome.find else {
            return;
        };
        let fragments = &self.edit.renderer.fragments;
        let (Some(first), Some(last)) = (
            fragments.iter().map(|f| f.source_range.0).min(),
            fragments.iter().map(|f| f.source_range.1).max(),
        ) else {
            return;
        };
        let matches = bar.find.matches();
        let from = matches.partition_point(|found| found.1 < first);
        let to = matches.partition_point(|found| found.0 <= last);
        let theme = crate::palette::ThemeExt::get_lb_theme(&self.edit.renderer.ctx);
        let other = theme.neutral_bg_tertiary();
        let current = theme.fg().yellow.lerp_to_gamma(theme.neutral_bg(), 0.5);
        for found in &matches[from..to.max(from)] {
            let color = if Some(*found) == bar.find.current() {
                current
            } else {
                other
            };
            self.edit.show_range(ui, *found, color);
        }
    }

    /// Whether the selection toolbar shows: an editable Markdown note with
    /// the keyboard and a selection, outside a pointer drag and an input
    /// method's composition. A press on the toolbar takes the keyboard
    /// from the text until the click lands, so it keeps showing then.
    fn selection_toolbar_shows(&self, ctx: &Context) -> bool {
        let selection = self.edit.renderer.buffer.current.selection;
        if !self.formattable()
            || selection.is_empty()
            || self.edit.in_progress_selection.is_some()
            || self.chrome.composing
        {
            return false;
        }
        let on_toolbar = self.chrome.selection_toolbar.is_some_and(|rect| {
            ctx.input(|input| {
                let pointer = &input.pointer;
                let at = pointer.press_origin().or(pointer.interact_pos());
                (pointer.any_down() || pointer.any_released())
                    && at.is_some_and(|at| rect.contains(at))
            })
        });
        self.focused(ctx) || on_toolbar
    }

    /// The small toolbar over a selection: bold, italic, underline,
    /// strikethrough and link.
    pub(crate) fn show_selection_toolbar(&mut self, ctx: &Context) {
        if !self.selection_toolbar_shows(ctx) {
            self.chrome.selection_toolbar = None;
            return;
        }
        let selection = self.edit.renderer.buffer.current.selection;
        let text_rect = self.chrome.text_rect.unwrap_or(Rect::EVERYTHING);
        let rects: Vec<Rect> = self
            .edit
            .range_rects((selection.start(), selection.end()))
            .into_iter()
            .filter(|rect| rect.intersects(text_rect))
            .collect();
        let (Some(top), Some(bottom)) = (
            rects.iter().min_by(|a, b| a.top().total_cmp(&b.top())),
            rects
                .iter()
                .max_by(|a, b| a.bottom().total_cmp(&b.bottom())),
        ) else {
            self.chrome.selection_toolbar = None;
            return;
        };
        // Above the selection, or below it when the text's top edge, with
        // the toolbar and find bar beyond it, leaves no room.
        let height = self
            .chrome
            .selection_toolbar
            .map_or(36.0, |rect| rect.height());
        let (pos, pivot) = if top.top() - height - 6.0 >= text_rect.top() {
            (Pos2::new(top.left(), top.top() - 6.0), Align2::LEFT_BOTTOM)
        } else {
            (
                Pos2::new(bottom.left(), bottom.bottom() + 6.0),
                Align2::LEFT_TOP,
            )
        };
        let apple = is_apple(ctx);
        let mut run = None;
        let tools = inline_tools().into_iter().chain([tool(
            Command::Format(Format::Link),
            Icon::Link,
            "Link",
        )]);
        let area = egui::Area::new(self.id().with("selection-toolbar"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .pivot(pivot)
            .show(ctx, |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    let builder = UiBuilder::new()
                        .accessibility_role(Role::Toolbar)
                        .accessibility_label("Selection formatting");
                    ui.scope_builder(builder, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for tool in tools {
                                let name = format!("{} selection", tool.name);
                                let hover = named_with_shortcut(tool.name, tool.command, apple);
                                let clicked = glyph_button(ui, &tool.glyph, None, true)
                                    .on_hover_text(hover)
                                    .accessible_name(name)
                                    .clicked();
                                if clicked {
                                    run = Some(tool.command);
                                }
                            }
                        });
                    });
                });
            });
        self.chrome.selection_toolbar = Some(area.response.rect);
        if let Some(command) = run {
            self.run_tool(ctx, command);
        }
    }
}
