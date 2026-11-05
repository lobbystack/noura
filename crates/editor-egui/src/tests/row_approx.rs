//! The scroll area's per-row height estimates, kept across edits that
//! reparse in place, match estimates made afresh.

use editor_core::input::{Advance, Increment};
use editor_core::offset_types::Grapheme;

use super::harness::TestEditor;
use crate::input::{Event, Location, Region};

#[test]
fn row_estimates_kept_across_in_place_edits_match_fresh_ones() {
    let note: String = (0..60)
        .map(|n| {
            format!(
                "## Part {n}\n\nText {n} with *some* words.\n\n- one\n  - two\n- [ ] task\n\n\
                 > quote {n}\n\n    code {n}\n\n"
            )
        })
        .collect();
    let mut ws = TestEditor::new(&note);
    ws.editor.edit.renderer.doc.set_large_note_bytes(0);
    ws.editor.edit.renderer.doc.hold_background_parses(true);
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };

    for step in 0..60 {
        let len = ws
            .editor
            .edit
            .renderer
            .buffer
            .current
            .segs
            .last_cursor_position()
            .0;
        let at = Grapheme(next(len + 1));
        ws.push(Event::Select {
            region: Region::Location(Location::Grapheme(at)),
        });
        ws.push(match next(4) {
            0 => Event::Newline { shift: false },
            1 => Event::Delete {
                region: Region::SelectionOrAdvance {
                    advance: Advance::By(Increment::Char),
                    backwards: true,
                },
            },
            2 => Event::Replace {
                region: Region::Selection,
                text: "\n\n- new\n".into(),
                advance_cursor: true,
            },
            _ => Event::Replace {
                region: Region::Selection,
                text: "x".into(),
                advance_cursor: true,
            },
        });
        ws.enter_frame();

        let renderer = &mut ws.editor.edit.renderer;
        let ast = renderer.take_ast();
        ast.with_root(|root| {
            let blocks: Vec<_> = root.children().collect();
            let kept: Vec<f32> = blocks
                .iter()
                .enumerate()
                .map(|(index, block)| renderer.row_approx(index, block, blocks.len()))
                .collect();
            renderer.layout_cache.clear();
            let fresh: Vec<f32> = blocks
                .iter()
                .map(|block| {
                    renderer.block_pre_spacing_height_approx(block)
                        + renderer.height_approx(block)
                        + renderer.block_post_spacing_height_approx(block)
                })
                .collect();
            assert_eq!(kept, fresh, "step {step}");
        });
        renderer.put_ast(ast);
    }
    assert!(ws.editor.edit.renderer.doc.parse_stats().in_place > 30);
}

/// An opening code fence in a large note turns everything after it into
/// code, past what an edit reparses in place: the editor draws the in-place
/// parse until the background parse arrives, then draws that.
#[test]
fn a_fence_opened_in_a_large_note_draws_before_and_after_the_full_parse() {
    let note: String = (0..3000)
        .map(|n| format!("Paragraph {n} of the note.\n\n"))
        .collect();
    let mut ws = TestEditor::new(&note);
    ws.editor.edit.renderer.doc.hold_background_parses(true);
    let at = note.find("Paragraph 1500 ").expect("the note holds it");
    ws.push(Event::Select {
        region: Region::Location(Location::Grapheme(Grapheme(at))),
    });
    ws.enter_frame();
    for typed in ["`", "`", "`"] {
        ws.push(Event::Replace {
            region: Region::Selection,
            text: typed.into(),
            advance_cursor: true,
        });
        ws.enter_frame();
    }
    ws.push(Event::Newline { shift: false });
    ws.enter_frame();
    ws.enter_frame();
    assert_eq!(ws.editor.edit.renderer.doc.parse_stats().installed, 0);

    ws.editor.edit.renderer.doc.hold_background_parses(false);
    assert!(
        ws.editor
            .edit
            .renderer
            .doc
            .wait_for_background_parse(std::time::Duration::from_secs(60))
    );
    ws.enter_frame();
    ws.enter_frame();
    let renderer = &mut ws.editor.edit.renderer;
    assert_eq!(renderer.doc.parse_stats().installed, 1);
    let ast = renderer.take_ast();
    let code_blocks = ast.with_root(|root| {
        root.children()
            .filter(|block| {
                matches!(
                    block.data.borrow().value,
                    comrak::nodes::NodeValue::CodeBlock(_)
                )
            })
            .count()
    });
    renderer.put_ast(ast);
    assert_eq!(code_blocks, 1, "the fence runs to the end of the note");
}

/// Nothing folds, so a Lockbook fold marker a file holds is text: moving
/// the caret, which changes the reveal state, keeps every height estimate.
#[test]
fn the_caret_keeps_height_estimates_in_a_note_with_a_fold_marker() {
    let mut note = String::from("# Plans <!-- {\"fold\":true} -->\n\n");
    for n in 0..40 {
        note.push_str(&format!("Paragraph {n} with *some* words.\n\n"));
    }
    let mut ws = TestEditor::new(&note);
    ws.enter_frame();

    let renderer = &mut ws.editor.edit.renderer;
    let ast = renderer.take_ast();
    ast.with_root(|root| {
        let blocks: Vec<_> = root.children().collect();
        for block in &blocks {
            renderer.height_approx(block);
        }
        renderer.doc.bump_reveal_seq();
        let kept = blocks
            .iter()
            .filter(|block| renderer.get_cached_node_height_approx(block).is_some())
            .count();
        assert_eq!(kept, blocks.len());
    });
    renderer.put_ast(ast);
}
