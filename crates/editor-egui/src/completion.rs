//! The files typing `[[` offers: a list that opens under the caret as you
//! type a link and narrows with each letter. Up and Down move through it,
//! Enter, Tab or a click puts the file in the link, and Escape closes it
//! until you type again. The embedding app finds the files through
//! [`LinkResolver::suggest`](crate::resolvers::LinkResolver::suggest), and
//! `editor-core` makes the edit.
//!
//! Adapted from the link completions of Lockbook's editor
//! (`tab/markdown_editor/widget/link_completions.rs`, Unlicense; see
//! `docs/provenance/lockbook-editor.md`): the list's state, its keys and
//! its place under the caret. The list draws with egui's own widgets.

use editor_core::resolvers::LinkSuggestion;
use egui::accesskit::Role;
use egui::{Align2, Atom, Button, Context, Frame, Key, Order, Pos2, Rect, RichText, UiBuilder};

use crate::Editor;
use crate::input::Event;

/// The list of files for the `[[` link being typed.
#[derive(Default)]
pub(crate) struct LinkCompletion {
    /// Typing opened the list, and nothing closed it since.
    open: bool,
    /// The highlighted file.
    selected: usize,
    /// The files offered, with what was typed and the resolver's sequence
    /// number when they were found.
    found: Option<(String, u64, Vec<LinkSuggestion>)>,
    /// Where the list drew last frame.
    rect: Option<Rect>,
    /// A file was just picked. The text the pick changes doesn't open the
    /// list again, even when the caret stays in the link, as it does
    /// before a `|display text` or `#heading`.
    picked: bool,
}

impl LinkCompletion {
    fn close(&mut self) {
        self.open = false;
        self.found = None;
        self.selected = 0;
    }
}

impl Editor {
    /// The files the list shows now: while it's open, the editor has the
    /// keyboard and the app offers at least one file. A press on the list
    /// takes the keyboard from the text until the click lands, so it keeps
    /// showing then.
    fn shown_suggestions(&self, ctx: &Context) -> Option<&[LinkSuggestion]> {
        let completion = &self.chrome.completion;
        let on_list = completion.rect.is_some_and(|rect| {
            ctx.input(|input| {
                let pointer = &input.pointer;
                let at = pointer.press_origin().or(pointer.interact_pos());
                (pointer.any_down() || pointer.any_released())
                    && at.is_some_and(|at| rect.contains(at))
            })
        });
        if !completion.open || !(self.focused(ctx) || on_list) {
            return None;
        }
        let (_, _, found) = completion.found.as_ref()?;
        (!found.is_empty()).then_some(found.as_slice())
    }

    /// Takes Up, Down, Enter, Tab and Escape while the list shows, before
    /// the text sees them. Only the keys alone: Shift-Up still extends the
    /// selection, and Alt-Enter still follows a link.
    pub(crate) fn handle_completion_keys(&mut self, ctx: &Context) {
        let Some(found) = self.shown_suggestions(ctx) else {
            return;
        };
        let count = found.len();
        let (up, down, enter, tab, escape) = ctx.input_mut(|input| {
            (
                consume_plain_key(input, Key::ArrowUp),
                consume_plain_key(input, Key::ArrowDown),
                consume_plain_key(input, Key::Enter),
                consume_plain_key(input, Key::Tab),
                consume_plain_key(input, Key::Escape),
            )
        });
        let picked = (enter || tab).then(|| found[self.chrome.completion.selected].link.clone());
        let completion = &mut self.chrome.completion;
        if up {
            completion.selected = completion.selected.checked_sub(1).unwrap_or(count - 1);
        }
        if down {
            completion.selected = (completion.selected + 1) % count;
        }
        if escape {
            completion.close();
        }
        if let Some(link) = picked {
            self.pick_link(ctx, link);
        }
        if up || down || escape {
            ctx.request_repaint();
        }
    }

    /// Puts `link` in the `[[` link being typed and closes the list.
    fn pick_link(&mut self, ctx: &Context, link: String) {
        self.chrome.completion.close();
        self.chrome.completion.picked = true;
        self.push_event(Event::CompleteLink { link });
        self.focus(ctx);
        ctx.request_repaint();
    }

    /// Opens the list when typing leaves the caret in a `[[` link, finds
    /// the files for what's typed, and closes the list once the caret
    /// leaves the link. A pick's edit lands by the next call, which leaves
    /// the list closed.
    pub(crate) fn update_completion(&mut self, text_updated: bool) {
        let picked = std::mem::take(&mut self.chrome.completion.picked);
        if !text_updated && !self.chrome.completion.open {
            return;
        }
        let Some(query) = self.edit.renderer.doc.link_query() else {
            self.chrome.completion.close();
            return;
        };
        if text_updated && !picked {
            self.chrome.completion.open = true;
        }
        if !self.chrome.completion.open {
            return;
        }
        let seq = self.edit.renderer.doc.link_resolver.seq();
        let completion = &mut self.chrome.completion;
        let fresh = completion
            .found
            .as_ref()
            .is_some_and(|(typed, found_at, _)| *typed == query.typed && *found_at == seq);
        if !fresh {
            let found = self.edit.renderer.doc.link_resolver.suggest(&query.typed);
            let completion = &mut self.chrome.completion;
            if completion
                .found
                .as_ref()
                .is_none_or(|(typed, ..)| *typed != query.typed)
            {
                completion.selected = 0;
            }
            completion.found = Some((query.typed, seq, found));
        }
        let completion = &mut self.chrome.completion;
        let count = completion
            .found
            .as_ref()
            .map_or(0, |(_, _, found)| found.len());
        completion.selected = completion.selected.min(count.saturating_sub(1));
    }

    /// The list, under the caret, or above it when the text's bottom edge
    /// leaves no room. Each file shows its name, and its folder (`/` at
    /// the top of the workspace) when another file shares the name.
    pub(crate) fn show_completion(&mut self, ctx: &Context) {
        let Some(found) = self.shown_suggestions(ctx).map(<[_]>::to_vec) else {
            self.chrome.completion.rect = None;
            return;
        };
        let caret = self.edit.renderer.buffer.current.selection.1;
        let Some([top, bottom]) = self.edit.cursor_line(caret) else {
            return;
        };
        let text_rect = self.chrome.text_rect.unwrap_or(Rect::EVERYTHING);
        let height = self
            .chrome
            .completion
            .rect
            .map_or(found.len() as f32 * 24.0 + 12.0, |rect| rect.height());
        let (pos, pivot) = if bottom.y + 4.0 + height <= text_rect.bottom() {
            (Pos2::new(top.x, bottom.y + 4.0), Align2::LEFT_TOP)
        } else {
            (Pos2::new(top.x, top.y - 4.0), Align2::LEFT_BOTTOM)
        };
        let selected = self.chrome.completion.selected;
        let mut clicked = None;
        let area = egui::Area::new(self.id().with("link-suggestions"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .pivot(pivot)
            .show(ctx, |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    let builder = UiBuilder::new()
                        .accessibility_role(Role::ListBox)
                        .accessibility_label("Link suggestions");
                    ui.scope_builder(builder, |ui| {
                        ui.set_min_width(240.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (index, suggestion) in found.iter().enumerate() {
                            let folder = folder_column(suggestion.folder.as_deref());
                            let button = Button::selectable(
                                index == selected,
                                (
                                    RichText::new(&suggestion.name),
                                    Atom::grow(),
                                    RichText::new(folder).weak(),
                                ),
                            )
                            .min_size(egui::vec2(ui.available_width(), 24.0));
                            let name = match suggestion.folder.as_deref() {
                                Some("") => {
                                    format!("{}, at the top of the workspace", suggestion.name)
                                }
                                Some(folder) => format!("{}, in {folder}", suggestion.name),
                                None => suggestion.name.clone(),
                            };
                            let row = ui.add(button).accessible_name(name);
                            ui.ctx().accesskit_node_builder(row.id, |node| {
                                node.set_role(Role::ListBoxOption);
                                node.set_selected(index == selected);
                            });
                            if row.clicked() {
                                clicked = Some(suggestion.link.clone());
                            }
                        }
                    });
                });
            });
        self.chrome.completion.rect = Some(area.response.rect);
        if let Some(link) = clicked {
            self.pick_link(ctx, link);
        }
    }
}

/// Consumes the presses of `key` made with no modifier held. egui's own
/// `consume_key` ignores an extra Shift or Alt.
fn consume_plain_key(input: &mut egui::InputState, key: Key) -> bool {
    let mut found = false;
    input.events.retain(|event| {
        let plain = matches!(
            event,
            egui::Event::Key {
                key: pressed,
                pressed: true,
                modifiers,
                ..
            } if *pressed == key && modifiers.is_none()
        );
        found |= plain;
        !plain
    });
    found
}

/// What the list shows beside a file's name: its folder when another file
/// shares the name, `/` for the top of the workspace, and nothing when
/// the name alone tells the file apart.
fn folder_column(folder: Option<&str>) -> &str {
    match folder {
        Some("") => "/",
        Some(folder) => folder,
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::folder_column;

    #[test]
    fn a_shared_name_at_the_top_of_the_workspace_shows_a_slash() {
        assert_eq!(folder_column(Some("")), "/");
        assert_eq!(folder_column(Some("Projects/2026")), "Projects/2026");
        assert_eq!(folder_column(None), "");
    }
}
