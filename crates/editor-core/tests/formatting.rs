//! Seam 2: the formatting commands the toolbar and the shortcuts run,
//! through `editor-core` alone. Ported from the legacy app's
//! `packages/editor/src/commands.test.ts`, whose rules they keep.

use editor_core::MdDoc;
use editor_core::commands::Format;
use editor_core::input::{Event, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;
use proptest::prelude::*;

fn g(offset: usize) -> Grapheme {
    Grapheme(offset)
}

/// A note holding `text` with `from..to` selected, the caret at `to`.
fn doc(text: &str, from: usize, to: usize) -> MdDoc {
    let mut doc = MdDoc::new(text);
    doc.apply(
        [Event::Select {
            region: Region::BetweenLocations {
                start: Location::Grapheme(g(from)),
                end: Location::Grapheme(g(to)),
            },
        }],
        &NoLayout,
    );
    doc
}

/// The text and the selection, smaller offset first, after `event` runs on
/// `text` with `from..to` selected. The fixtures are ASCII, so offsets
/// count bytes.
fn run(text: &str, from: usize, to: usize, event: Event) -> (String, usize, usize) {
    let mut doc = doc(text, from, to);
    doc.apply([event], &NoLayout);
    let (anchor, caret) = doc.selection();
    (
        doc.text().to_owned(),
        anchor.0.min(caret.0),
        anchor.0.max(caret.0),
    )
}

fn text_after(text: &str, from: usize, to: usize, event: Event) -> String {
    run(text, from, to, event).0
}

fn format(format: Format) -> Event {
    Event::Format(format)
}

#[test]
fn bold_wraps_the_selection_and_unwraps_it() {
    assert_eq!(text_after("hello", 0, 5, format(Format::Bold)), "**hello**");
    assert_eq!(text_after("**hello**", 0, 9, format(Format::Bold)), "hello");
}

#[test]
fn italic_and_strikethrough_wrap() {
    assert_eq!(text_after("hi", 0, 2, format(Format::Italic)), "*hi*");
    assert_eq!(
        text_after("hi", 0, 2, format(Format::Strikethrough)),
        "~~hi~~"
    );
}

#[test]
fn underline_wraps_with_inline_html() {
    assert_eq!(
        text_after("hi", 0, 2, format(Format::Underline)),
        "<u>hi</u>"
    );
    assert_eq!(
        text_after("<u>hi</u>", 0, 9, format(Format::Underline)),
        "hi"
    );
}

#[test]
fn a_heading_applies_and_replaces_the_level() {
    assert_eq!(
        text_after("text", 0, 4, format(Format::Heading(1))),
        "# text"
    );
    assert_eq!(
        text_after("# existing", 0, 10, format(Format::Heading(2))),
        "## existing"
    );
    assert_eq!(
        text_after("## existing", 0, 11, format(Format::Heading(0))),
        "existing"
    );
}

#[test]
fn a_quote_prefix_toggles() {
    assert_eq!(
        text_after("line", 0, 4, format(Format::BlockQuote)),
        "> line"
    );
    assert_eq!(
        text_after("> line", 0, 6, format(Format::BlockQuote)),
        "line"
    );
}

#[test]
fn bold_on_an_empty_selection_puts_the_caret_between_the_markers() {
    assert_eq!(
        run("abc ", 4, 4, format(Format::Bold)),
        ("abc ****".to_owned(), 6, 6)
    );
}

#[test]
fn bold_again_on_the_empty_pair_removes_it() {
    assert_eq!(
        run("abc ****", 6, 6, format(Format::Bold)),
        ("abc ".to_owned(), 4, 4)
    );
}

#[test]
fn bold_keeps_the_wrapped_text_selected_so_it_toggles_back() {
    let wrapped = run("hello", 0, 5, format(Format::Bold));
    assert_eq!(wrapped, ("**hello**".to_owned(), 2, 7));
    assert_eq!(text_after(&wrapped.0, 2, 7, format(Format::Bold)), "hello");
}

#[test]
fn toggling_off_works_when_only_the_visible_text_is_selected() {
    assert_eq!(
        text_after("a **bold** b", 4, 8, format(Format::Bold)),
        "a bold b"
    );
    assert_eq!(
        text_after("a *it* b", 3, 5, format(Format::Italic)),
        "a it b"
    );
    assert_eq!(
        text_after("a ~~x~~ b", 4, 5, format(Format::Strikethrough)),
        "a x b"
    );
    assert_eq!(text_after("a `x` b", 3, 4, format(Format::Code)), "a x b");
    assert_eq!(
        text_after("a <u>x</u> b", 5, 6, format(Format::Underline)),
        "a x b"
    );
}

#[test]
fn italic_on_bold_text_adds_italic_instead_of_stripping_bold() {
    assert_eq!(
        text_after("**bold**", 0, 8, format(Format::Italic)),
        "***bold***"
    );
    assert_eq!(
        text_after("**bold**", 2, 6, format(Format::Italic)),
        "***bold***"
    );
}

#[test]
fn bold_and_italic_toggle_independently_on_bold_italic_text() {
    assert_eq!(text_after("***x***", 3, 4, format(Format::Bold)), "*x*");
    assert_eq!(text_after("***x***", 3, 4, format(Format::Italic)), "**x**");
}

#[test]
fn a_link_takes_selected_text_as_its_label_and_a_selected_url_as_its_target() {
    // Selected text becomes the label; the caret waits in the target.
    assert_eq!(
        run("see docs", 4, 8, format(Format::Link)),
        ("see [docs]()".to_owned(), 11, 11)
    );
    // A selected URL becomes the target; the caret waits in the label.
    assert_eq!(
        run("https://a.b", 0, 11, format(Format::Link)),
        ("[](https://a.b)".to_owned(), 1, 1)
    );
    assert_eq!(
        run("", 0, 0, format(Format::Link)),
        ("[]()".to_owned(), 1, 1)
    );
}

#[test]
fn a_numbered_list_numbers_each_selected_line() {
    assert_eq!(
        text_after("a\nb\nc", 0, 5, format(Format::NumberedList)),
        "1. a\n2. b\n3. c"
    );
}

#[test]
fn switching_list_kinds_replaces_the_marker() {
    assert_eq!(text_after("1. a", 0, 4, format(Format::BulletList)), "- a");
    assert_eq!(
        text_after("- a", 0, 3, format(Format::CheckList)),
        "- [ ] a"
    );
    assert_eq!(text_after("- [ ] a", 0, 7, format(Format::CheckList)), "a");
}

#[test]
fn a_line_format_keeps_the_caret_in_the_same_text() {
    // The caret after "a" stays after "a" once the marker goes in.
    assert_eq!(
        run("a", 1, 1, format(Format::BulletList)),
        ("- a".to_owned(), 3, 3)
    );
    assert_eq!(
        run("text", 2, 2, format(Format::Heading(1))),
        ("# text".to_owned(), 4, 4)
    );
}

#[test]
fn mod_enter_checks_a_task_or_turns_a_line_into_one() {
    assert_eq!(
        text_after("- [ ] a", 0, 0, Event::ToggleTaskLine),
        "- [x] a"
    );
    assert_eq!(
        text_after("- [x] a", 0, 0, Event::ToggleTaskLine),
        "- [ ] a"
    );
    assert_eq!(text_after("- a", 0, 0, Event::ToggleTaskLine), "- [ ] a");
    assert_eq!(text_after("1. a", 0, 0, Event::ToggleTaskLine), "1. [ ] a");
    assert_eq!(text_after("a", 0, 0, Event::ToggleTaskLine), "- [ ] a");
}

#[test]
fn mod_enter_flips_every_kind_of_task_marker() {
    assert_eq!(
        text_after(
            "* [ ] a\n+ [x] b\n1. [ ] c\n- [ ]",
            0,
            26,
            Event::ToggleTaskLine
        ),
        "* [x] a\n+ [ ] b\n1. [x] c\n- [x]"
    );
}

#[test]
fn tab_nests_an_ordered_item_under_the_item_above_and_restarts_it_at_one() {
    assert_eq!(
        text_after(
            "1. parent\n2. child\n",
            12,
            12,
            Event::Indent { deindent: false }
        ),
        "1. parent\n   1. child\n"
    );
}

#[test]
fn tab_nests_a_bullet_with_its_children() {
    assert_eq!(
        text_after(
            "- a\n- b\n  - c\n- d",
            5,
            5,
            Event::Indent { deindent: false }
        ),
        "- a\n  - b\n    - c\n- d"
    );
}

#[test]
fn shift_tab_lifts_an_item_and_its_children_out_one_level() {
    assert_eq!(
        text_after("- a\n  - b\n", 7, 7, Event::Indent { deindent: true }),
        "- a\n- b\n"
    );
    assert_eq!(
        text_after(
            "- a\n  - b\n    - c\n",
            7,
            7,
            Event::Indent { deindent: true }
        ),
        "- a\n- b\n  - c\n"
    );
}

#[test]
fn tab_keeps_the_caret_in_the_item_it_moves() {
    assert_eq!(
        run("- a\n- b", 7, 7, Event::Indent { deindent: false }),
        ("- a\n  - b".to_owned(), 9, 9)
    );
}

#[test]
fn tab_outside_a_list_changes_no_text_and_leaves_the_key_to_the_window() {
    let mut note = doc("text", 2, 2);
    assert!(!note.tab_indents());
    note.apply([Event::Indent { deindent: false }], &NoLayout);
    assert_eq!(note.text(), "text");

    // Inside a list, Tab is the editor's even where it can't nest.
    assert!(doc("- first", 3, 3).tab_indents());
    assert_eq!(
        text_after("- first", 3, 3, Event::Indent { deindent: false }),
        "- first"
    );
}

#[test]
fn the_toolbar_reads_the_formats_at_the_selection() {
    assert!(doc("a **b** c", 4, 5).formats().contains(&Format::Bold));
    assert!(
        doc("- [ ] task", 8, 8)
            .formats()
            .contains(&Format::CheckList)
    );
    assert!(
        doc("a <u>x</u> b", 5, 6)
            .formats()
            .contains(&Format::Underline)
    );
    assert!(doc("see [docs](x)", 6, 6).formats().contains(&Format::Link));
    assert!(
        doc("> quoted", 4, 4)
            .formats()
            .contains(&Format::BlockQuote)
    );
    assert!(doc("abc ****", 6, 6).formats().contains(&Format::Bold));
    assert_eq!(doc("plain", 2, 2).formats(), Vec::<Format>::new());
}

#[test]
fn the_paragraph_style_follows_the_caret_line() {
    assert_eq!(doc("# one", 3, 3).paragraph_style(), 1);
    assert_eq!(doc("text\n### three", 8, 8).paragraph_style(), 3);
    assert_eq!(doc("plain", 2, 2).paragraph_style(), 0);
}

#[test]
fn one_command_undoes_in_one_step() {
    let mut note = doc("a\nb\nc", 0, 5);
    note.apply([format(Format::NumberedList)], &NoLayout);
    assert_eq!(note.text(), "1. a\n2. b\n3. c");

    note.apply([Event::Undo], &NoLayout);
    assert_eq!(note.text(), "a\nb\nc");
    assert_eq!(note.selection(), (g(0), g(5)));

    note.apply([Event::Redo], &NoLayout);
    assert_eq!(note.text(), "1. a\n2. b\n3. c");
}

#[test]
fn a_read_only_note_ignores_formatting() {
    let mut note = doc("hello", 0, 5);
    note.readonly = true;
    note.apply(
        [
            format(Format::Bold),
            Event::ToggleTaskLine,
            Event::Indent { deindent: false },
        ],
        &NoLayout,
    );
    assert_eq!(note.text(), "hello");
}

#[test]
fn formatting_text_with_wide_characters_counts_graphemes() {
    // "é" is two bytes and "👍🏽" one grapheme of eight bytes.
    assert_eq!(
        run("é 👍🏽", 2, 3, format(Format::Bold)),
        ("é **👍🏽**".to_owned(), 4, 5)
    );
    assert_eq!(
        run("👍🏽", 1, 1, format(Format::CheckList)),
        ("- [ ] 👍🏽".to_owned(), 7, 7)
    );
}

fn any_command() -> impl Strategy<Value = Event> {
    prop::sample::select(vec![
        Event::Format(Format::Bold),
        Event::Format(Format::Italic),
        Event::Format(Format::Underline),
        Event::Format(Format::Strikethrough),
        Event::Format(Format::Code),
        Event::Format(Format::Link),
        Event::Format(Format::BulletList),
        Event::Format(Format::NumberedList),
        Event::Format(Format::CheckList),
        Event::Format(Format::BlockQuote),
        Event::Format(Format::Heading(0)),
        Event::Format(Format::Heading(3)),
        Event::ToggleTaskLine,
        Event::Indent { deindent: false },
        Event::Indent { deindent: true },
    ])
}

/// Lines with list, quote and heading markers, Unicode whitespace in their
/// indents, and inline syntax, wide and combining characters after them.
fn any_note() -> impl Strategy<Value = String> {
    let indent = prop::sample::select(&["", "  ", "\t", "\u{3000}", " \u{a0}", "    "][..]);
    let marker = prop::sample::select(
        &[
            "", "- ", "1. ", "2) ", "> ", "# ", "* ", "- [ ] ", "1. [x] ",
        ][..],
    );
    let content = prop::collection::vec(
        prop::sample::select(
            &[
                "a", "é", "日", "e\u{301}", "*", "**", "_", "~~", "`", "<u>", "</u>", "[", "](",
                ")", " ",
            ][..],
        ),
        0..5,
    );
    prop::collection::vec((indent, marker, content), 0..6).prop_map(|lines| {
        lines
            .into_iter()
            .map(|(indent, marker, content)| format!("{indent}{marker}{}", content.concat()))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

proptest! {
    /// Every command runs on any text and selection, and one undo puts the
    /// text back.
    #[test]
    fn commands_run_on_any_text_and_undo_in_one_step(
        text in any_note(),
        start in 0.0..=1.0f64,
        end in 0.0..=1.0f64,
        command in any_command(),
    ) {
        let last = editor_core::unicode_segs::calc(&text).last_cursor_position().0;
        let at = |fraction: f64| (last as f64 * fraction).round() as usize;
        let mut note = doc(&text, at(start), at(end));
        note.apply([command], &NoLayout);
        if note.text() != text {
            note.apply([Event::Undo], &NoLayout);
            prop_assert_eq!(note.text(), text.as_str());
        }
    }
}
