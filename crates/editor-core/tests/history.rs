//! The buffer's history: which edits undo together, and how outside edits
//! merge with the user's. Ported from the tests in Lockbook's
//! `lb-rs/src/model/text/buffer.rs` (BSD-3-Clause); see
//! `docs/provenance/lockbook-editor.md`.

use std::time::{Duration, Instant};

use editor_core::buffer::Buffer;
use editor_core::offset_types::{Grapheme, IntoRangeExt as _, RangeExt as _};
use editor_core::operation_types::{Operation, Replace};

/// One frame of input, queued at `at`.
fn frame_at(buf: &mut Buffer, ops: Vec<Operation>, at: Instant) {
    buf.queue_at(ops, at);
    buf.update();
}

fn frame(buf: &mut Buffer, ops: Vec<Operation>) {
    frame_at(buf, ops, Instant::now());
}

/// Types `s` at the caret. The selection names the caret's old offset, and
/// the insertion carries it forward.
fn type_str_at(buf: &mut Buffer, s: &str, at: Instant) {
    let cursor = buf.current.selection.start();
    frame_at(
        buf,
        vec![
            Operation::Replace(Replace {
                range: cursor.into_range(),
                text: s.into(),
            }),
            Operation::Select(cursor.into_range()),
        ],
        at,
    );
}

fn type_str(buf: &mut Buffer, s: &str) {
    type_str_at(buf, s, Instant::now());
}

/// Return with no list or quote to continue: a single `\n`.
fn enter(buf: &mut Buffer) {
    type_str(buf, "\n");
}

fn backspace(buf: &mut Buffer) {
    let cursor = buf.current.selection.start();
    if cursor.0 == 0 {
        return;
    }
    let prev = Grapheme(cursor.0 - 1);
    frame(
        buf,
        vec![
            Operation::Replace(Replace {
                range: (prev, cursor),
                text: "".into(),
            }),
            Operation::Select(prev.into_range()),
        ],
    );
}

fn click(buf: &mut Buffer, pos: usize) {
    frame(buf, vec![Operation::Select(Grapheme(pos).into_range())]);
}

/// Several replacements in one frame, the shape Tab makes over several
/// lines.
fn cmd_indent_lines(buf: &mut Buffer, line_starts: &[usize]) {
    let mut ops: Vec<Operation> = line_starts
        .iter()
        .map(|&s| {
            Operation::Replace(Replace {
                range: Grapheme(s).into_range(),
                text: "  ".into(),
            })
        })
        .collect();
    ops.push(Operation::Select(Grapheme(line_starts[0] + 2).into_range()));
    frame(buf, ops);
}

fn cmd_deindent_lines(buf: &mut Buffer, line_starts: &[usize]) {
    let mut ops: Vec<Operation> = line_starts
        .iter()
        .map(|&s| {
            Operation::Replace(Replace {
                range: (Grapheme(s), Grapheme(s + 2)),
                text: "".into(),
            })
        })
        .collect();
    ops.push(Operation::Select(Grapheme(line_starts[0]).into_range()));
    frame(buf, ops);
}

fn type_into_selection(buf: &mut Buffer, text: &str) {
    let range = buf.current.selection;
    frame(
        buf,
        vec![
            Operation::Replace(Replace {
                range,
                text: text.into(),
            }),
            Operation::Select((range.start(), range.start())),
        ],
    );
}

#[test]
fn type_into_forward_selection() {
    let mut buffer = Buffer::from("hello");
    buffer.current.selection = (Grapheme(0), Grapheme(5));
    type_into_selection(&mut buffer, "X");
    assert_eq!(buffer.current.text(), "X");
    assert_eq!(buffer.current.selection, (Grapheme(1), Grapheme(1)));
}

#[test]
fn type_into_backward_selection() {
    let mut buffer = Buffer::from("hello");
    buffer.current.selection = (Grapheme(5), Grapheme(0));
    type_into_selection(&mut buffer, "X");
    assert_eq!(buffer.current.text(), "X");
    assert_eq!(buffer.current.selection, (Grapheme(1), Grapheme(1)));
}

#[test]
fn single_command_one_unit() {
    let mut buf = Buffer::from("ab\ncd\nef\n");
    cmd_indent_lines(&mut buf, &[0, 3, 6]);
    assert_eq!(buf.current.text(), "  ab\n  cd\n  ef\n");
    buf.undo();
    assert_eq!(buf.current.text(), "ab\ncd\nef\n");
}

#[test]
fn two_commands_two_units() {
    let mut buf = Buffer::from("ab\ncd\nef\n");
    cmd_indent_lines(&mut buf, &[0, 3, 6]);
    cmd_indent_lines(&mut buf, &[2, 7, 12]);
    let after_two = buf.current.text().to_string();
    buf.undo();
    assert_ne!(buf.current.text(), after_two);
    assert_eq!(buf.current.text(), "  ab\n  cd\n  ef\n");
    buf.undo();
    assert_eq!(buf.current.text(), "ab\ncd\nef\n");
}

#[test]
fn click_only_doesnt_create_unit() {
    let mut buf = Buffer::from("hello");
    click(&mut buf, 3);
    // The click alone mustn't stop the next edit's undo from reaching
    // past it.
    type_str(&mut buf, "X");
    assert_eq!(buf.current.text(), "helXlo");
    buf.undo();
    assert_eq!(buf.current.text(), "hello");
}

#[test]
fn click_between_commands_absorbs() {
    let mut buf = Buffer::from("ab\ncd\nef\n");
    cmd_indent_lines(&mut buf, &[0, 3, 6]);
    click(&mut buf, 0);
    cmd_indent_lines(&mut buf, &[2, 7, 12]);
    buf.undo();
    assert_eq!(buf.current.text(), "  ab\n  cd\n  ef\n");
    buf.undo();
    assert_eq!(buf.current.text(), "ab\ncd\nef\n");
}

#[test]
fn rapid_typing_one_unit() {
    let mut buf = Buffer::from("");
    type_str(&mut buf, "a");
    type_str(&mut buf, "b");
    type_str(&mut buf, "c");
    assert_eq!(buf.current.text(), "abc");
    buf.undo();
    assert_eq!(buf.current.text(), "");
}

#[test]
fn typing_long_pause_splits() {
    // Lockbook's test slept 600 ms between "b" and "c"; these keystrokes
    // carry their times instead.
    let start = Instant::now();
    let mut buf = Buffer::from("");
    type_str_at(&mut buf, "a", start);
    type_str_at(&mut buf, "b", start + Duration::from_millis(50));
    type_str_at(&mut buf, "c", start + Duration::from_millis(650));
    type_str_at(&mut buf, "d", start + Duration::from_millis(700));
    assert_eq!(buf.current.text(), "abcd");
    buf.undo();
    assert_eq!(buf.current.text(), "ab");
    buf.undo();
    assert_eq!(buf.current.text(), "");
}

#[test]
fn type_cmd_type_three_units() {
    let mut buf = Buffer::from("xx\nyy\n");
    // The caret at the end, so typing appends.
    buf.current.selection = (Grapheme(6), Grapheme(6));
    type_str(&mut buf, "a");
    type_str(&mut buf, "b");
    cmd_indent_lines(&mut buf, &[0, 3]);
    type_str(&mut buf, "c");
    type_str(&mut buf, "d");
    let final_text = buf.current.text().to_string();
    buf.undo();
    assert_ne!(buf.current.text(), final_text);
    assert_eq!(buf.current.text(), "  xx\n  yy\nab");
    buf.undo();
    assert_eq!(buf.current.text(), "xx\nyy\nab");
    buf.undo();
    assert_eq!(buf.current.text(), "xx\nyy\n");
}

#[test]
fn type_enter_type_one_unit() {
    let mut buf = Buffer::from("");
    type_str(&mut buf, "a");
    type_str(&mut buf, "b");
    enter(&mut buf);
    type_str(&mut buf, "c");
    type_str(&mut buf, "d");
    assert_eq!(buf.current.text(), "ab\ncd");
    buf.undo();
    assert_eq!(buf.current.text(), "");
}

#[test]
fn backspace_groups_with_typing() {
    let mut buf = Buffer::from("");
    type_str(&mut buf, "a");
    type_str(&mut buf, "b");
    backspace(&mut buf);
    type_str(&mut buf, "c");
    assert_eq!(buf.current.text(), "ac");
    buf.undo();
    assert_eq!(buf.current.text(), "");
}

#[test]
fn paste_own_unit() {
    let mut buf = Buffer::from("");
    type_str(&mut buf, "a");
    type_str(&mut buf, "b");
    // A paste is one replacement with several graphemes.
    type_str(&mut buf, "PASTED");
    type_str(&mut buf, "c");
    type_str(&mut buf, "d");
    assert_eq!(buf.current.text(), "abPASTEDcd");
    buf.undo();
    assert_eq!(buf.current.text(), "abPASTED");
    buf.undo();
    assert_eq!(buf.current.text(), "ab");
    buf.undo();
    assert_eq!(buf.current.text(), "");
}

#[test]
fn select_overwrite_groups() {
    let mut buf = Buffer::from("hello");
    // Selecting "ell" and typing "X" replaces three graphemes with one in
    // one frame.
    buf.current.selection = (Grapheme(1), Grapheme(4));
    frame(
        &mut buf,
        vec![
            Operation::Replace(Replace {
                range: (Grapheme(1), Grapheme(4)),
                text: "X".into(),
            }),
            Operation::Select(Grapheme(2).into_range()),
        ],
    );
    type_str(&mut buf, "Y");
    assert_eq!(buf.current.text(), "hXYo");
    buf.undo();
    assert_eq!(buf.current.text(), "hello");
}

#[test]
fn click_then_edit_undoes_to_post_click_cursor() {
    let mut buf = Buffer::from("hello world");
    click(&mut buf, 6);
    type_str(&mut buf, "X");
    assert_eq!(buf.current.text(), "hello Xworld");
    buf.undo();
    assert_eq!(buf.current.text(), "hello world");
    // The caret goes back to the click, not to where it was before it.
    assert_eq!(buf.current.selection, (Grapheme(6), Grapheme(6)));
}

#[test]
fn round_trip_two_units() {
    let mut buf = Buffer::from("  foo\n  bar\n");
    click(&mut buf, 2);
    cmd_deindent_lines(&mut buf, &[0, 6]);
    let mid = buf.current.text().to_string();
    assert_eq!(mid, "foo\nbar\n");
    cmd_indent_lines(&mut buf, &[0, 4]);
    assert_eq!(buf.current.text(), "  foo\n  bar\n");
    buf.undo();
    assert_eq!(buf.current.text(), mid);
    buf.undo();
    assert_eq!(buf.current.text(), "  foo\n  bar\n");
}

// ── Merging outside edits ──
//
// Lockbook's `Buffer::merge(a, b)` merged two versions of a base text and
// stayed behind. Typing `a` into the editor and then loading `b` from disk
// queues the same operations against the same base, so these tests drive
// that path instead.

/// Edits `base` into `local` in the editor, then merges `outside` from disk.
fn merge(base: &str, local: &str, outside: &str) -> String {
    let mut buffer = Buffer::from(base);
    let ops = editor_core::diff::diff(base, local)
        .into_iter()
        .map(Operation::Replace)
        .collect();
    buffer.queue(ops);
    buffer.update();
    buffer.reload(outside.into());
    buffer.update();
    buffer.current.text().to_string()
}

#[test]
fn buffer_merge_nonintersecting_replace() {
    let base = "base content base";
    let local = "local content base";
    let remote = "base content remote";
    assert_eq!(merge(base, local, remote), "local content remote");
    assert_eq!(merge(base, remote, local), "local content remote");
}

#[test]
fn buffer_merge_prefix_replace() {
    assert_eq!(
        merge("base content", "local content", "remote content"),
        "local content"
    );
}

#[test]
fn buffer_merge_infix_replace() {
    let base = "con base tent";
    let local = "con local tent";
    let remote = "con remote tent";
    assert_eq!(merge(base, local, remote), "con local tent");
    assert_eq!(merge(base, remote, local), "con remote tent");
}

#[test]
fn buffer_merge_postfix_replace() {
    let base = "content base";
    let local = "content local";
    let remote = "content remote";
    assert_eq!(merge(base, local, remote), "content local");
    assert_eq!(merge(base, remote, local), "content remote");
}

#[test]
fn buffer_merge_insert() {
    let base = "content";
    let local = "content local";
    let remote = "content remote";
    assert_eq!(merge(base, local, remote), "content local remote");
    assert_eq!(merge(base, remote, local), "content remote local");
}

/// Documents behavior more than it asserts a goal.
#[test]
fn buffer_merge_insert_replace() {
    let base = "content";
    let local = "content local";
    let remote = "remote";
    assert_eq!(merge(base, local, remote), "remote");
    assert_eq!(merge(base, remote, local), "remote local");
}

/// This case once crashed Lockbook's merge.
#[test]
fn buffer_merge_crash() {
    let base = "con tent";
    let local = "cont tent locallocal";
    let remote = "cont remote tent";
    merge(base, local, remote);
    merge(base, remote, local);
}

// ── Outside edits after a save ──
//
// A save records the version that held the file's text. Undo back past that
// version, then a new edit, drops it; merging an outside edit into unsaved
// edits leaves no version that held the file's text. Later outside edits
// must still merge with the text on screen.

/// Records a save of the buffer's current text, as the editor's save does.
fn save(buf: &mut Buffer) -> String {
    let text = buf.current.rope().to_string();
    buf.saved(buf.current.seq, text.clone());
    text
}

fn reload(buf: &mut Buffer, outside: &str) {
    buf.reload(outside.into());
    buf.update();
}

/// Replaces graphemes `start..end` with `text` in one frame and puts the
/// caret after it.
fn replace_range(buf: &mut Buffer, start: usize, end: usize, text: &str) {
    frame(
        buf,
        vec![
            Operation::Select((Grapheme(start), Grapheme(end))),
            Operation::Replace(Replace {
                range: (Grapheme(start), Grapheme(end)),
                text: text.into(),
            }),
        ],
    );
}

#[test]
fn an_outside_edit_after_undo_past_the_save_merges_with_the_restored_text() {
    let mut buf = Buffer::from("Keep this line\nSecond line here\n");
    replace_range(&mut buf, 0, 15, "");
    assert_eq!(save(&mut buf), "Second line here\n");
    buf.undo();
    assert_eq!(buf.current.text(), "Keep this line\nSecond line here\n");

    reload(&mut buf, "2nd line here\n");

    assert_eq!(buf.current.text(), "Keep this line\n2nd line here\n");
}

#[test]
fn an_outside_edit_after_undoing_the_saved_typing_keeps_the_undo() {
    let mut buf = Buffer::from("alpha beta\n");
    replace_range(&mut buf, 10, 10, " gamma delta");
    save(&mut buf);
    buf.undo();

    reload(&mut buf, "intro alpha beta gamma delta\n");

    assert_eq!(buf.current.text(), "intro alpha beta\n");
}

#[test]
fn an_outside_edit_after_undo_and_new_typing_merges_both() {
    let mut buf = Buffer::from("alpha beta\n");
    replace_range(&mut buf, 10, 10, " gamma");
    save(&mut buf);
    buf.undo();
    // The new edit drops the saved version from the history.
    replace_range(&mut buf, 10, 10, " delta");
    assert!(!buf.can_redo());

    reload(&mut buf, "intro alpha beta gamma\n");

    assert_eq!(buf.current.text(), "intro alpha beta delta\n");
}

#[test]
fn an_outside_edit_pending_through_an_undo_merges_with_the_restored_text() {
    let mut buf = Buffer::from("one two\n");
    replace_range(&mut buf, 3, 7, "");
    save(&mut buf);

    buf.reload("one\nthree\n".into());
    buf.undo();
    buf.update();

    assert_eq!(buf.current.text(), "one two\nthree\n");
}

#[test]
fn redo_back_to_the_save_merges_through_the_history() {
    let mut buf = Buffer::from("alpha\n");
    replace_range(&mut buf, 5, 5, " beta");
    save(&mut buf);
    buf.undo();
    buf.redo();

    reload(&mut buf, "alpha beta gamma\n");

    assert_eq!(buf.current.text(), "alpha beta gamma\n");
    // Nothing unsaved: the merged text is the file's, so a second outside
    // edit diffs against it.
    reload(&mut buf, "start alpha beta gamma\n");
    assert_eq!(buf.current.text(), "start alpha beta gamma\n");
}

#[test]
fn a_second_outside_edit_merges_after_the_first_merged_into_unsaved_typing() {
    let mut buf = Buffer::from("hello world\n");
    replace_range(&mut buf, 0, 0, "A ");

    reload(&mut buf, "hello world!!\n");
    assert_eq!(buf.current.text(), "A hello world!!\n");

    reload(&mut buf, "hello there world!!\n");
    assert_eq!(buf.current.text(), "A hello there world!!\n");
}

#[test]
fn an_unchanged_file_keeps_redo() {
    let mut buf = Buffer::from("alpha\n");
    replace_range(&mut buf, 5, 5, " beta");
    save(&mut buf);
    buf.undo();

    reload(&mut buf, "alpha beta\n");

    assert!(buf.can_redo());
    assert_eq!(buf.current.text(), "alpha\n");
}

/// Graphemes for random edits: ASCII, multi-byte letters, emoji sequences
/// with joiners, skin tones and flags, and combining marks.
const POOL: &[&str] = &[
    "a",
    "b",
    "z",
    " ",
    "\n",
    "\t",
    "\u{e9}",
    "\u{f1}",
    "\u{fc}",
    "\u{65e5}",
    "\u{672c}",
    "\u{8a9e}",
    "\u{1f44b}",
    "\u{1f389}",
    "\u{1f525}",
    "\u{2764}\u{fe0f}",
    "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}",
    "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}",
    "\u{1f44d}\u{1f3fd}",
    "\u{1f1fa}\u{1f1f8}",
    "\u{1f1ef}\u{1f1f5}",
    "e\u{301}",
    "a\u{308}",
];

/// A small deterministic generator, standing in for Lockbook's seeded rand.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }

    fn pick(&mut self) -> &'static str {
        POOL[self.below(POOL.len())]
    }
}

/// One random edit of `doc`: delete up to five graphemes, insert up to five,
/// replace up to five with up to three, or clear it.
fn random_edit(rng: &mut Rng, doc: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation as _;
    let mut g: Vec<String> = doc.graphemes(true).map(String::from).collect();
    let len = g.len();
    match rng.below(4) {
        0 if len > 0 => {
            let pos = rng.below(len);
            let del = 1 + rng.below((len - pos).min(5));
            g.drain(pos..pos + del);
        }
        2 if len > 0 => {
            let pos = rng.below(len);
            let del = 1 + rng.below((len - pos).min(5));
            let ins: Vec<String> = (0..1 + rng.below(3)).map(|_| rng.pick().into()).collect();
            g.splice(pos..pos + del, ins);
        }
        3 if len > 0 => g.clear(),
        _ => {
            let pos = rng.below(len + 1);
            for j in 0..1 + rng.below(5) {
                g.insert(pos + j, rng.pick().into());
            }
        }
    }
    g.concat()
}

#[test]
fn buffer_merge_fuzz() {
    let mut rng = Rng(42);
    let bases = [
        "hello world",
        "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}\u{1f1fa}\u{1f1f8}\u{1f525}",
        "caf\u{e9} \u{f1}o\u{f1}o \u{65e5}\u{672c}\u{8a9e}",
        "",
    ];
    for _ in 0..10_000 {
        let base = bases[rng.below(bases.len())];
        let a = random_edit(&mut rng, base);
        let b = random_edit(&mut rng, base);
        // Must not panic.
        merge(base, &a, &b);
        merge(base, &b, &a);
    }
}
