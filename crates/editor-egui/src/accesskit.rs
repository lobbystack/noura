//! The note in the accessibility tree: one multi-line text node whose value
//! is the note's text, with text runs a screen reader reads and the caret
//! and selection it announces.
//!
//! Modelled on egui's `text_selection::accesskit_text` (one text run per
//! visual row, at most 255 characters each, with character lengths,
//! positions, widths and word starts), which this can't call: it reads an
//! egui `Galley`, and the editor lays its text out with glyphon. Rows come
//! from the fragments the frame painted. Lines the frame didn't lay out
//! still get runs, with their text but no geometry, so the runs always
//! spell out the whole note and selections anywhere in it resolve.

use egui::accesskit::{self, Role};
use egui::{Id, Sense, Ui, UiBuilder};

use crate::MdRender;
use crate::widget::utils::wrap_layout::FragmentContent;
use editor_core::offset_types::{Grapheme, RangeExt as _};

/// AccessKit's word starts are `u8` indices, so a run holds at most this
/// many characters when it reports them.
const MAX_CHARS_PER_RUN: usize = 255;

/// Where one grapheme drew: its x span and its row.
#[derive(Clone, Copy)]
struct Placed {
    x: f32,
    width: f32,
    top: f32,
    bottom: f32,
}

/// One text run: a stretch of the note's text.
struct Run {
    /// Byte range in the note.
    bytes: std::ops::Range<usize>,
    /// The byte offset where each character starts, relative to the run.
    char_starts: Vec<usize>,
    /// Each character's x and width in screen coordinates, when the frame
    /// drew the run.
    geometry: Option<Vec<(f32, f32)>>,
    /// The row it drew on.
    rows: Option<(f32, f32)>,
    /// Whether the previous run continues the same visual row.
    continues_row: bool,
}

impl MdRender {
    /// Reports the note to AccessKit as the node `id`: a multi-line text
    /// input called `label` whose value is the note's text, with text runs as children and
    /// the selection. Does nothing while AccessKit is off.
    pub(crate) fn update_accesskit(&self, ui: &mut Ui, id: Id, label: &str) {
        let text = self.buffer.current.text();
        let reported = ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::MultilineTextInput);
            node.set_label(label);
            node.set_value(text);
            if self.readonly {
                node.set_read_only();
            }
        });
        if reported.is_none() {
            return;
        }

        let runs = self.accesskit_runs();
        for (index, run) in runs.iter().enumerate() {
            let run_id = id.with(("text-run", index));
            let builder = UiBuilder::new()
                .scope_id(run_id)
                .accessibility_parent(id)
                .accessibility_role(Role::TextRun)
                .sense(Sense::empty());
            let _run_ui = ui.new_child(builder);
            ui.ctx().accesskit_node_builder(run_id, |node| {
                self.fill_run(node, run, text, index, id, &runs);
            });
        }

        let position = |offset: Grapheme| {
            let byte = self.buffer.current.segs.offset_to_byte(offset).0;
            let index = runs
                .iter()
                .position(|run| run.bytes.start <= byte && byte < run.bytes.end)
                .unwrap_or(runs.len().saturating_sub(1));
            let run = &runs[index];
            let into = byte.saturating_sub(run.bytes.start);
            let character_index = match run.char_starts.binary_search(&into) {
                Ok(index) => index,
                Err(index) => index,
            };
            accesskit::TextPosition {
                node: id.with(("text-run", index)).accesskit_id(),
                character_index,
            }
        };
        if runs.is_empty() {
            return;
        }
        let (anchor, focus) = self.buffer.current.selection;
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_text_selection(accesskit::TextSelection {
                anchor: position(anchor),
                focus: position(focus),
            });
        });
    }

    fn fill_run(
        &self,
        node: &mut accesskit::Node,
        run: &Run,
        text: &str,
        index: usize,
        id: Id,
        runs: &[Run],
    ) {
        let value = &text[run.bytes.clone()];
        node.set_text_direction(accesskit::TextDirection::LeftToRight);
        node.set_value(value);
        let lengths: Vec<u8> = run
            .char_starts
            .iter()
            .zip(run.char_starts.iter().skip(1).chain([&value.len()]))
            .map(|(start, end)| (end - start) as u8)
            .collect();
        node.set_character_lengths(lengths);

        if run.char_starts.len() <= MAX_CHARS_PER_RUN {
            let mut word_starts = Vec::new();
            let mut previous_word = false;
            for (character, start) in run.char_starts.iter().enumerate() {
                let word = value[*start..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric);
                if word && !previous_word && character > 0 {
                    word_starts.push(character as u8);
                }
                previous_word = word;
            }
            node.set_word_starts(word_starts);
        }

        if let (Some(geometry), Some((top, bottom))) = (&run.geometry, run.rows) {
            let x0 = geometry.first().map_or(0.0, |(x, _)| *x);
            let x1 = geometry
                .iter()
                .map(|(x, width)| x + width)
                .fold(x0, f32::max);
            node.set_bounds(accesskit::Rect {
                x0: x0.into(),
                y0: top.into(),
                x1: x1.into(),
                y1: bottom.into(),
            });
            node.set_character_positions(geometry.iter().map(|(x, _)| x - x0).collect::<Vec<_>>());
            node.set_character_widths(geometry.iter().map(|(_, width)| *width).collect::<Vec<_>>());
        }
        if run.continues_row && index > 0 {
            node.set_previous_on_line(id.with(("text-run", index - 1)).accesskit_id());
        }
        if runs.get(index + 1).is_some_and(|next| next.continues_row) {
            node.set_next_on_line(id.with(("text-run", index + 1)).accesskit_id());
        }
    }

    /// Where each grapheme the frame drew sits, by grapheme offset.
    fn placed_graphemes(&self) -> std::collections::HashMap<usize, Placed> {
        let mut placed = std::collections::HashMap::new();
        for fragment in &self.fragments {
            let (start, end) = (fragment.source_range.start(), fragment.source_range.end());
            if start == end {
                continue;
            }
            let (top, bottom) = (fragment.rect.top(), fragment.rect.bottom());
            match &fragment.content {
                FragmentContent::Glyphs {
                    cluster_advances, ..
                } if !fragment.atomic && cluster_advances.len() == end.0 - start.0 => {
                    let mut x = fragment.rect.left() + fragment.content_inset.left;
                    for (offset, advance) in (start.0..end.0).zip(cluster_advances) {
                        placed.insert(
                            offset,
                            Placed {
                                x,
                                width: *advance,
                                top,
                                bottom,
                            },
                        );
                        x += advance;
                    }
                }
                _ => {
                    // An atom such as an image or a footnote mark: its
                    // first grapheme spans it.
                    for offset in start.0..end.0 {
                        let first = offset == start.0;
                        placed.entry(offset).or_insert(Placed {
                            x: if first {
                                fragment.rect.left()
                            } else {
                                fragment.rect.right()
                            },
                            width: if first { fragment.rect.width() } else { 0.0 },
                            top,
                            bottom,
                        });
                    }
                }
            }
        }
        placed
    }

    /// The characters of the graphemes in `range`: each one's grapheme
    /// and starting byte in the note. A grapheme is one character, unless
    /// it is too long to report as one, when each of its scalars is.
    fn accesskit_characters(&self, range: (Grapheme, Grapheme)) -> Vec<(usize, usize)> {
        let segs = &self.buffer.current.segs;
        let text = self.buffer.current.text();
        let mut out = Vec::with_capacity(range.1.0.saturating_sub(range.0.0));
        for offset in range.0.0..range.1.0 {
            let start = segs.offset_to_byte(Grapheme(offset)).0;
            let end = segs.offset_to_byte(Grapheme(offset + 1)).0;
            if end - start <= usize::from(u8::MAX) {
                out.push((offset, start));
            } else {
                out.extend(
                    text[start..end]
                        .char_indices()
                        .map(|(at, _)| (offset, start + at)),
                );
            }
        }
        out
    }

    /// A run of text the frame didn't draw.
    fn undrawn_run(&self, range: (Grapheme, Grapheme)) -> Run {
        let segs = &self.buffer.current.segs;
        let base = segs.offset_to_byte(range.0).0;
        Run {
            bytes: base..segs.offset_to_byte(range.1).0,
            char_starts: self
                .accesskit_characters(range)
                .into_iter()
                .map(|(_, start)| start - base)
                .collect(),
            geometry: None,
            rows: None,
            continues_row: false,
        }
    }

    /// The note's text as runs: one per drawn row of each line the frame
    /// laid out, at most [`MAX_CHARS_PER_RUN`] characters each, and one per
    /// stretch of lines it didn't.
    fn accesskit_runs(&self) -> Vec<Run> {
        use editor_core::bounds::RangesExt as _;

        let lines = &self.bounds.source_lines;
        let last = self.buffer.current.segs.last_cursor_position();
        let placed = self.placed_graphemes();
        let mut drawn = vec![false; lines.len()];
        for fragment in &self.fragments {
            let (first, end) = lines.find_containing(fragment.source_range.start(), true, true);
            if first < end {
                drawn[first] = true;
            }
        }

        let mut runs: Vec<Run> = Vec::new();
        let mut undrawn_from: Option<Grapheme> = None;
        for (index, line) in lines.iter().enumerate() {
            // The line and the line break after it.
            let next = lines.get(index + 1).map_or(last, |next| next.0);
            if !drawn[index] {
                undrawn_from.get_or_insert(line.0);
                continue;
            }
            if let Some(from) = undrawn_from.take() {
                runs.push(self.undrawn_run((from, line.0)));
            }
            self.push_line_runs(&mut runs, (line.0, next), &placed);
        }
        if let Some(from) = undrawn_from {
            runs.push(self.undrawn_run((from, last)));
        }
        if runs.is_empty() {
            // An empty note still has a place for the caret.
            runs.push(self.undrawn_run((Grapheme(0), Grapheme(0))));
        }
        runs
    }

    /// Runs for one drawn line and its line break, split where its drawn
    /// graphemes change rows. A grapheme that drew nothing, such as hidden
    /// syntax, sits where the next drawn one starts, or after the last.
    fn push_line_runs(
        &self,
        runs: &mut Vec<Run>,
        range: (Grapheme, Grapheme),
        placed: &std::collections::HashMap<usize, Placed>,
    ) {
        let segs = &self.buffer.current.segs;
        let line_start = self
            .fragments
            .iter()
            .find(|fragment| fragment.source_range.start() >= range.0);
        let mut run = Run {
            bytes: segs.offset_to_byte(range.0).0..segs.offset_to_byte(range.0).0,
            char_starts: Vec::new(),
            geometry: Some(Vec::new()),
            rows: line_start.map(|fragment| (fragment.rect.top(), fragment.rect.bottom())),
            continues_row: false,
        };
        let mut row_top: Option<f32> = None;
        let mut end_x = line_start.map_or(0.0, |fragment| fragment.rect.left());
        // Characters that drew nothing yet, waiting for a position.
        let mut pending: Vec<usize> = Vec::new();

        fn push(run: &mut Run, start: usize, x: f32, width: f32) {
            run.char_starts.push(start - run.bytes.start);
            if let Some(geometry) = &mut run.geometry {
                geometry.push((x, width));
            }
        }
        fn split(runs: &mut Vec<Run>, run: &mut Run, at: usize, same_row: bool) {
            let next = Run {
                bytes: at..at,
                char_starts: Vec::new(),
                geometry: Some(Vec::new()),
                rows: None,
                continues_row: same_row,
            };
            let mut done = std::mem::replace(run, next);
            done.bytes.end = at;
            if !done.char_starts.is_empty() {
                runs.push(done);
            }
        }

        for (grapheme, start) in self.accesskit_characters(range) {
            let Some(place) = placed.get(&grapheme).copied() else {
                if run.char_starts.len() + pending.len() >= MAX_CHARS_PER_RUN {
                    for pending_start in pending.drain(..) {
                        push(&mut run, pending_start, end_x, 0.0);
                    }
                    split(runs, &mut run, start, true);
                }
                pending.push(start);
                continue;
            };
            let first = pending.first().copied().unwrap_or(start);
            let new_row = row_top.is_some_and(|top| (top - place.top).abs() > 0.5);
            if new_row || run.char_starts.len() + pending.len() >= MAX_CHARS_PER_RUN {
                split(runs, &mut run, first, !new_row);
            }
            for pending_start in pending.drain(..) {
                push(&mut run, pending_start, place.x, 0.0);
            }
            push(&mut run, start, place.x, place.width);
            row_top = Some(place.top);
            run.rows = Some(run.rows.map_or((place.top, place.bottom), |(top, bottom)| {
                (top.min(place.top), bottom.max(place.bottom))
            }));
            end_x = place.x + place.width;
        }
        for pending_start in pending.drain(..) {
            push(&mut run, pending_start, end_x, 0.0);
        }
        run.bytes.end = segs.offset_to_byte(range.1).0;
        if !run.char_starts.is_empty() {
            runs.push(run);
        }
    }
}
