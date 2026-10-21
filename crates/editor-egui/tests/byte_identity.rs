//! Opening a note in the editor, displaying it and closing it leaves its
//! bytes identical. Trial S1 checks generated notes, the workspace format's
//! conformance fixtures and the Markdown from the legacy editor's tests.
//!
//! Opening reads the note's bytes as Noura does: the byte-order mark and
//! CRLF line endings go into a layout, and the editor gets LF text.
//! "Closing" asks the editor for the text a save would write and writes it
//! with that layout, which must give the bytes the note was opened from.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, OnceLock};

use editor_egui::{Editor, MdConfig};
use egui::{Pos2, RawInput, Rect, Vec2};
use glyphon::FontSystem;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestCaseError};
use workspace_format::frontmatter_end;
use workspace_format::raw_text::{compose_raw_bytes, split_raw_bytes};

/// One font system for every editor the tests open: building one per note
/// would dominate the run time.
fn font_system() -> Arc<Mutex<FontSystem>> {
    static FONTS: OnceLock<Arc<Mutex<FontSystem>>> = OnceLock::new();
    Arc::clone(FONTS.get_or_init(|| {
        Arc::new(Mutex::new(FontSystem::new_with_locale_and_db(
            "en-US".into(),
            editor_egui::fonts::default_font_database(),
        )))
    }))
}

/// Runs one headless frame showing `editor` in a window of `size`.
fn frame(ctx: &egui::Context, editor: &mut Editor, size: Vec2) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        ..Default::default()
    };
    ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            editor.show(ui);
        });
    })
    .drop_without_applying_deltas();
}

/// What opening, displaying and closing a note did to it.
#[derive(Debug, PartialEq)]
enum Outcome {
    Identical,
    /// Closing would write these bytes instead.
    Changed(Vec<u8>),
    /// The editor panicked with this message.
    Panicked(String),
}

/// Shows `text` in a new editor in a window, then in a window tall enough
/// to lay out every block, and returns the text a save would write.
fn display(ctx: &egui::Context, text: &str) -> String {
    let mut editor = Editor::new(ctx, text, MdConfig::default());
    frame(ctx, &mut editor, Vec2::new(800.0, 600.0));
    frame(ctx, &mut editor, Vec2::new(800.0, 600.0));
    let tall = 600.0 + 40.0 * text.lines().count() as f32;
    frame(ctx, &mut editor, Vec2::new(800.0, tall));
    let (_, saved) = editor.flush_for_save();
    saved
}

/// Opens `note`'s bytes the way Noura opens a note, displays it and closes
/// it. Opening reads the byte-order mark and CRLF line endings into a
/// layout and hands the editor LF text; closing writes the editor's text
/// back with that layout. A note with frontmatter opens a second time the
/// way a managed note does, with only the text after its frontmatter in
/// the editor.
fn open_display_close(note: &str) -> Outcome {
    let bytes = note.as_bytes();
    let (text, layout) = split_raw_bytes(bytes).expect("the note is UTF-8");
    let result = catch_unwind(AssertUnwindSafe(|| {
        let ctx = egui::Context::default();
        editor_egui::install_font_system(&ctx, font_system());
        let written = compose_raw_bytes(&display(&ctx, &text), &layout);
        match frontmatter_end(&text) {
            Some(end) if written == bytes => {
                let body = display(&ctx, &text[end..]);
                compose_raw_bytes(&format!("{}{body}", &text[..end]), &layout)
            }
            _ => written,
        }
    }));
    match result {
        Ok(written) if written == bytes => Outcome::Identical,
        Ok(written) => Outcome::Changed(written),
        Err(panic) => Outcome::Panicked(
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_default(),
        ),
    }
}

/// Callouts after frontmatter that holds a lone carriage return, and
/// callouts near other unusual line endings. comrak's front matter
/// extension counts the frontmatter's lines by LF alone, while the rest of
/// comrak and editor-core's source lines also end a line at a lone CR, so
/// the editor reads every construct after such frontmatter one line off
/// until ticket 18 replaces that detection. A callout there used to panic
/// in `alert_type_title_ranges`; it now draws its whole first line as its
/// title, the note keeps its bytes, and the generators produce such notes.
#[test]
fn callouts_after_unusual_line_endings_keep_their_bytes() {
    for note in [
        "---\na\rb\n---\n> [!note] A\n",
        "---\ra: 1\n---\n# Heading\n\n> [!tip] T\n> body\n",
        "---\r\na: 1\r\n---\r\n> [!note] A\r\n",
        "a\rb\n> [!note] A\n",
        "\u{feff}# Heading\n",
        "\u{feff}\n> [!note] a\n",
        // Noura reads the byte-order mark off before the editor sees the
        // text, so a callout on the first line reads its own type.
        "\u{feff}> [!note] a\n",
    ] {
        assert_eq!(open_display_close(note), Outcome::Identical, "{note:?}");
    }
}

/// Typing can bring a note shown in the live preview into that class:
/// `---` and a line break at its start turn a first line that holds a lone
/// carriage return into frontmatter. The editor draws the callout after it
/// from the wrong line, without a panic.
#[test]
fn typing_a_note_into_misread_frontmatter_draws_without_a_panic() {
    use editor_egui::input::{Event, Location, Region};

    let ctx = egui::Context::default();
    editor_egui::install_font_system(&ctx, font_system());
    let mut editor = Editor::new(&ctx, "a\rb\n---\n> [!note] A\n", MdConfig::default());
    frame(&ctx, &mut editor, Vec2::new(800.0, 600.0));

    editor.push_event(Event::Select {
        region: Region::Location(Location::Grapheme(editor_core::offset_types::Grapheme(0))),
    });
    for typed in ["-", "-", "-", "\n"] {
        editor.push_event(Event::Replace {
            region: Region::Selection,
            text: typed.into(),
            advance_cursor: true,
        });
    }
    for _ in 0..3 {
        frame(&ctx, &mut editor, Vec2::new(800.0, 600.0));
    }

    assert_eq!(editor.text(), "---\na\rb\n---\n> [!note] A\n");
}

/// Asserts that every note in `notes` keeps its bytes, naming each that
/// doesn't.
fn assert_all_identical(notes: impl IntoIterator<Item = (String, String)>) {
    let failures: Vec<String> = notes
        .into_iter()
        .filter_map(|(name, note)| match open_display_close(&note) {
            Outcome::Identical => None,
            outcome => Some(format!("{name}: {note:?} -> {outcome:?}")),
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// Fixtures

fn read_fixture(name: &str) -> serde_json::Value {
    let path = format!(
        "{}/../../docs/workspace-format/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(&path).expect("the fixture exists"))
        .expect("the fixture is JSON")
}

/// A frontmatter value as a YAML scalar: plain when YAML reads it back as
/// the same string, quoted otherwise.
fn yaml_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text)
            if !text.is_empty()
                && text
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || " -_.:/@+".contains(ch))
                && !text.starts_with(['-', ' ', ':'])
                && !text.ends_with(' ')
                && !text.contains(": ")
                && !matches!(text.as_str(), "null" | "true" | "false") =>
        {
            text.clone()
        }
        other => other.to_string(),
    }
}

/// A note whose frontmatter holds `properties`, written the way the
/// workspace format writes it.
fn note_with_frontmatter(properties: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut note = String::from("---\n");
    for (key, value) in properties {
        match value {
            serde_json::Value::Array(items) if !items.is_empty() => {
                note.push_str(&format!("{key}:\n"));
                for item in items {
                    note.push_str(&format!("  - {}\n", yaml_scalar(item)));
                }
            }
            serde_json::Value::Array(_) => note.push_str(&format!("{key}: []\n")),
            serde_json::Value::Object(_) => note.push_str(&format!("{key}: {value}\n")),
            scalar => note.push_str(&format!("{key}: {}\n", yaml_scalar(scalar))),
        }
    }
    note.push_str("---\n\nBody text.\n");
    note
}

/// Notes built from `conformance-v1.json`, `queries-v1.json` and the text
/// fixtures shared with Yjs.
fn conformance_notes() -> Vec<(String, String)> {
    let mut notes = Vec::new();
    let conformance = read_fixture("conformance-v1.json");
    for (section, cases) in conformance.as_object().unwrap() {
        for case in cases.as_array().into_iter().flatten() {
            let name = format!("conformance-v1 {section}: {}", case["name"]);
            if let Some(properties) = case["value"].as_object() {
                notes.push((name.clone(), note_with_frontmatter(properties)));
            }
            for field in ["body", "expected_body", "file_ends_with"] {
                if let Some(text) = case[field].as_str() {
                    notes.push((format!("{name} {field}"), text.to_string()));
                }
            }
        }
    }

    let queries = read_fixture("queries-v1.json");
    for file in queries["files"].as_array().unwrap() {
        if let Some(content) = file["content"].as_str() {
            notes.push((format!("queries-v1 {}", file["path"]), content.to_string()));
        }
    }

    for (fixture, fields) in [
        ("yrs-text-v1.json", ["text", "baseText"]),
        ("yjs-text-v1.json", ["text", "text"]),
    ] {
        let value = read_fixture(fixture);
        for field in fields {
            notes.push((
                format!("{fixture} {field}"),
                value[field].as_str().unwrap().to_string(),
            ));
        }
        for (index, state) in value["states"].as_array().into_iter().flatten().enumerate() {
            notes.push((
                format!("{fixture} states[{index}]"),
                state.as_str().unwrap().to_string(),
            ));
        }
    }
    notes
}

#[test]
fn the_format_conformance_fixtures_keep_their_bytes() {
    let notes = conformance_notes();
    assert!(notes.len() > 60, "found {} notes", notes.len());
    assert_all_identical(notes);
}

/// The Markdown the legacy editor's tests open, with the test it comes from.
const LEGACY_NOTES: &[(&str, &str)] = &[
    (
        "packages/editor/src/preview-view.test.ts KITCHEN_SINK",
        "---\ntitle: Kitchen sink\ntags: [a, b]\n---\n\n# Heading with **bold** and *italic*\n\n\
         > [!note] Callout title\n> Callout body with [[Link|alias]] and #tag\n\n\
         > quote line one\n> quote line two\n\n\
         - [ ] open task\n* [x] done task\n1. [ ] ordered task\n- [ ]\n   1. nested\n\n\
         ```ts\nconst answer: number = 42; // comment\n```\n\n$$\nx^2 + y^2 = z^2\n$$\n\n\
         Inline $a+b$ and `code` and ==highlight== and %%hidden%% and <u>u</u>.\n\n\
         ![Alt](pic.png \"Title\") ![[photo.jpg|200]] ![[Other note]]\n\n\
         [a link](https://example.com) and https://bare.example.com\n\n\
         | A | B |\n| - | - |\n| 1 | 2 |\n\nSetext\n===\n\n---\n\nLast line ^block-id\n",
    ),
    (
        "packages/editor/src/preview-view.test.ts task",
        "intro\n\n- [ ] task\n",
    ),
    (
        "packages/editor/src/preview-view.test.ts links",
        "intro\n\nSee [[Plan|the plan]] and [docs](https://example.com)\n",
    ),
    (
        "packages/editor/src/editor-view.test.ts SAMPLE",
        "# Plan\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n$$E = mc^2$$\n\n![[Notes/index]]\n\n\
         Inline `[[Note]]` code\n",
    ),
    (
        "packages/editor/src/editor-view.test.ts PDF embeds",
        "![[lecture.pdf#page=7]]\n\n![](lecture.pdf#page=9)\n\n[Read](lecture.pdf#page=11)\n\n\
         Inline ![[lecture.pdf#page=3]] end\n",
    ),
    (
        "packages/editor/src/collaborative-view.test.ts",
        "Introduction\n\n[Lecture](lecture.pdf#page=7)\n",
    ),
    (
        "packages/editor/src/factory.test.ts lone carriage return",
        "a\rb\nc\r\n",
    ),
    (
        "packages/editor/src/factory.test.ts outside edit",
        "line one\nline two\nline three\nexternal\n",
    ),
    (
        "packages/editor/src/preview.test.ts table",
        "| A | B |\n| - | - |\n| 1 | 2 |\n",
    ),
    (
        "packages/editor/src/preview.test.ts display math",
        "Intro\n\n$$x + y$$\n\nOutro\n",
    ),
    (
        "packages/editor/src/preview.test.ts embeds",
        "![[Notes/index]]\n\nprose ![[Note]] tail\n",
    ),
    (
        "packages/editor/src/preview.test.ts padded embed",
        "  ![[Note]]  \n",
    ),
    (
        "packages/editor/src/preview.test.ts code span",
        "Fenced-looking `[[Note]] $x$ ==h== #tag %%c%%` prose\n",
    ),
    (
        "packages/editor/src/preview.test.ts hidden comment",
        "before %%secret **bold**%% after\n",
    ),
    (
        "packages/editor/src/preview.test.ts inline math",
        "$x$ here\n",
    ),
    (
        "packages/editor/src/preview.test.ts frontmatter",
        "---\ntitle: Plan\ntags: [a, \"#b\"]\nlink: \"[[Note]]\"\n---\n\nBody #tag\n",
    ),
    (
        "packages/editor/src/preview.test.ts unclosed frontmatter",
        "---\ntext\n",
    ),
    (
        "packages/editor/src/preview.test.ts multi-line math",
        "Intro\n\n$$\nx^2 + y^2\n$$\n\nOutro\n",
    ),
    (
        "packages/editor/src/preview.test.ts math emphasis",
        "$$\na*b*c #x\n$$\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts blockquote",
        "> one\n> two\n> three\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts callout",
        "> [!note] Title\n> body\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts folded callout",
        "> [!warning]- Careful\n> body\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts fenced code",
        "```js\nconst a = 1;\n```\n\nafter\n",
    ),
    (
        "packages/editor/src/preview.test.ts long emphasis",
        "**xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx**\n\nnext\n",
    ),
    (
        "packages/editor/src/preview.test.ts images",
        "![Alt](pic.png \"A title\")\n\n![[photo.jpg|300]]\n\nend\n",
    ),
    (
        "packages/editor/src/preview.test.ts link",
        "See [the docs](https://example.com) now\n",
    ),
    (
        "packages/editor/src/preview.test.ts tasks",
        "- [ ] one\n* [x] two\n+ [ ] three\n1. [X] four\n- [ ]\n\nend\n",
    ),
    (
        "packages/editor/src/commands.test.ts tasks",
        "- [ ] one\n- [x] two",
    ),
    (
        "packages/editor/src/commands.test.ts mixed tasks",
        "* [ ] a\n+ [x] b\n1. [ ] c\n- [ ]",
    ),
    (
        "packages/editor/src/commands.test.ts empty bold",
        "abc ****",
    ),
    ("packages/editor/src/commands.test.ts bold", "a **bold** b"),
    ("packages/editor/src/commands.test.ts italic", "a *it* b"),
    ("packages/editor/src/commands.test.ts strike", "a ~~x~~ b"),
    ("packages/editor/src/commands.test.ts code", "a `x` b"),
    (
        "packages/editor/src/commands.test.ts underline",
        "a <u>x</u> b",
    ),
    (
        "packages/editor/src/commands.test.ts bold italic",
        "***x***",
    ),
    (
        "packages/editor/src/commands.test.ts empty link",
        "see [docs]()",
    ),
    ("packages/editor/src/commands.test.ts lines", "a\nb\nc"),
    (
        "packages/editor/src/commands.test.ts numbered",
        "1. a\n2. b\n3. c",
    ),
    (
        "packages/editor/src/commands.test.ts ordered nesting",
        "1. parent\n2. child\n",
    ),
    (
        "packages/editor/src/commands.test.ts nesting",
        "- a\n- b\n  - c\n- d",
    ),
    (
        "packages/editor/src/text-sync.test.ts",
        "one\n2\nthree\nfour\n",
    ),
    (
        "apps/app/src/lib/editor/document-session.test.ts trailing space",
        "Hello \nworld",
    ),
    (
        "apps/app/src/lib/editor/managed-draft-session.test.ts",
        "outside\nline\none two",
    ),
];

#[test]
fn the_legacy_editor_fixtures_keep_their_bytes() {
    assert_all_identical(
        LEGACY_NOTES
            .iter()
            .map(|(name, note)| (name.to_string(), note.to_string())),
    );
}

#[test]
#[cfg_attr(debug_assertions, ignore = "takes minutes in a debug build")]
fn the_large_note_keeps_its_bytes() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/large-note.md");
    let note = std::fs::read_to_string(path).unwrap();
    assert_eq!(open_display_close(&note), Outcome::Identical);
}

// ---------------------------------------------------------------------------
// Generated notes

/// Inline text: words, Markdown the editor draws, syntax it doesn't know,
/// and the whitespace and characters a save must keep.
fn inline() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        4 => "[a-zA-Z]{1,8}".prop_map(String::from),
        1 => Just("__strong__".to_string()),
        1 => Just("**strong**".to_string()),
        1 => Just("_em_".to_string()),
        1 => Just("*em*".to_string()),
        1 => Just("`code`".to_string()),
        1 => Just("<!-- comment -->".to_string()),
        1 => Just("\u{a0}".to_string()),
        1 => Just("\t".to_string()),
        1 => Just("  ".to_string()),
        1 => Just("[link](https://example.com \"title\")".to_string()),
        1 => Just("<https://example.com>".to_string()),
        1 => Just("[[Wiki|alias]]".to_string()),
        1 => Just("![[embed.png|200]]".to_string()),
        1 => Just("![image](missing.png)".to_string()),
        1 => Just("==mark==".to_string()),
        1 => Just("~~strike~~".to_string()),
        1 => Just("<u>under</u>".to_string()),
        1 => Just("$x^2$".to_string()),
        1 => Just("#tag".to_string()),
        1 => Just("%%hidden%%".to_string()),
        1 => Just("{{template}}".to_string()),
        1 => Just("^block-id".to_string()),
        1 => Just("[^1]".to_string()),
        1 => Just("\\*escaped\\*".to_string()),
        1 => Just("&nbsp;&amp;".to_string()),
        1 => Just(":smile:".to_string()),
        1 => Just(">!spoiler!<".to_string()),
        1 => Just("<span class=\"x\">".to_string()),
        1 => Just("é日本👍🏽e\u{301}".to_string()),
    ];
    prop::collection::vec(piece, 1..8).prop_map(|pieces| pieces.join(" "))
}

/// A block's lines, without line endings.
fn block() -> impl Strategy<Value = Vec<String>> {
    prop_oneof![
        3 => prop::collection::vec(inline(), 1..4),
        1 => (1..=6usize, inline()).prop_map(|(level, text)| vec![format!("{} {text}", "#".repeat(level))]),
        1 => (inline(), prop_oneof![Just("==="), Just("---"), Just("=")])
            .prop_map(|(text, rule)| vec![text, rule.to_string()]),
        2 => prop::collection::vec(
            (
                prop_oneof![Just(""), Just("  "), Just("    "), Just("\t")],
                prop_oneof![Just("- "), Just("* "), Just("+ "), Just("1. "), Just("2) "),
                            Just("- [ ] "), Just("- [x] "), Just("* [X] ")],
                inline(),
            )
                .prop_map(|(indent, marker, text)| format!("{indent}{marker}{text}")),
            1..5,
        ),
        1 => prop::collection::vec(
            (prop_oneof![Just("> "), Just(">"), Just("> > "), Just("> [!note] "), Just("> [!unknown]- ")], inline())
                .prop_map(|(marker, text)| format!("{marker}{text}")),
            1..4,
        ),
        1 => (prop_oneof![Just("```"), Just("~~~"), Just("```rust"), Just("``` js")],
              prop::collection::vec("[ -~\t]{0,20}", 0..4))
            .prop_map(|(fence, body)| {
                let close = if fence.starts_with('~') { "~~~" } else { "```" };
                let mut lines = vec![fence.to_string()];
                lines.extend(body);
                lines.push(close.to_string());
                lines
            }),
        1 => prop::collection::vec("    [ -~]{0,20}", 1..3),
        1 => (1..4usize, 1..4usize).prop_map(|(columns, rows)| {
            let row = |cell: &str| format!("|{}", format!(" {cell} |").repeat(columns));
            let mut lines = vec![row("head"), format!("|{}", "---|".repeat(columns))];
            lines.extend((0..rows).map(|_| row("cell")));
            lines
        }),
        1 => prop_oneof![
            Just(vec!["<!-- a comment -->".to_string()]),
            Just(vec!["<!--".to_string(), "spans lines".to_string(), "-->".to_string()]),
            Just(vec!["<div align=\"center\">".to_string(), "html".to_string(), "</div>".to_string()]),
            Just(vec!["<details><summary>More</summary>".to_string(), "</details>".to_string()]),
        ],
        1 => prop_oneof![Just("---"), Just("***"), Just("___"), Just("- - -")].prop_map(|rule| vec![rule.to_string()]),
        1 => Just(vec!["$$".to_string(), "x^2 + y^2".to_string(), "$$".to_string()]),
        1 => prop_oneof![
            Just(":::note"), Just("[^1]: A footnote."), Just("Term\n: Definition"),
            Just("@[toc]"), Just("[TOC]"), Just("<custom-element/>"), Just("+++"),
            Just("%%"), Just("|"), Just("\\"), Just("<"), Just("`"), Just("```"),
        ].prop_map(|text| text.split('\n').map(String::from).collect()),
        2 => Just(vec![String::new()]),
    ]
}

/// Frontmatter lines, opening and closing rule included.
fn frontmatter() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(
        prop_oneof![
            Just("tags: [a, b]"),
            Just("title: \"A: title\""),
            Just("empty:"),
            Just("list:\n  - one\n  - two"),
            Just("id: note_01j00000000000000000000000"),
            Just("date: 2026-10-20"),
            Just("# a YAML comment"),
            Just("text: trailing  "),
            Just("tab:\tvalue"),
        ],
        0..4,
    )
    .prop_map(|properties| {
        let mut lines = vec!["---".to_string()];
        lines.extend(
            properties
                .iter()
                .flat_map(|p| p.split('\n'))
                .map(String::from),
        );
        lines.push("---".to_string());
        lines
    })
}

#[derive(Clone, Copy, Debug)]
enum LineEndings {
    Lf,
    Crlf,
    /// Each line ends in LF, CRLF or a lone CR.
    Mixed,
}

/// A note: an optional byte-order mark, optional frontmatter, blocks, each
/// line ended per `endings`, and an optional final line ending.
fn note() -> impl Strategy<Value = String> {
    (
        any::<bool>(),
        prop::option::of(frontmatter()),
        prop::collection::vec(block(), 0..8),
        prop_oneof![
            Just(LineEndings::Lf),
            Just(LineEndings::Crlf),
            Just(LineEndings::Mixed)
        ],
        prop::collection::vec(0..3usize, 64),
        any::<bool>(),
    )
        .prop_map(
            |(bom, frontmatter, blocks, endings, mixed, final_newline)| {
                let mut lines: Vec<String> = frontmatter.unwrap_or_default();
                for block in blocks {
                    lines.extend(block);
                    lines.push(String::new());
                }
                let mut note = String::new();
                if bom {
                    note.push('\u{feff}');
                }
                for (index, line) in lines.iter().enumerate() {
                    note.push_str(line);
                    let last = index + 1 == lines.len();
                    if last && !final_newline {
                        break;
                    }
                    note.push_str(match endings {
                        LineEndings::Lf => "\n",
                        LineEndings::Crlf => "\r\n",
                        LineEndings::Mixed => ["\n", "\r\n", "\r"][mixed[index % mixed.len()]],
                    });
                }
                note
            },
        )
}

/// Any short string over characters that matter to Markdown and to line
/// handling, for the cases structure doesn't reach.
fn scramble() -> impl Strategy<Value = String> {
    let characters: Vec<char> = "ab #*_`~[]()!<>|-+=:$%^&\\/\"' \t\n\r\u{a0}\u{feff}é日👍"
        .chars()
        .collect();
    prop::collection::vec(prop::sample::select(characters), 0..120)
        .prop_map(|characters| characters.into_iter().collect())
}

fn check(note: &str) -> Result<(), TestCaseError> {
    match open_display_close(note) {
        Outcome::Identical => Ok(()),
        outcome => Err(TestCaseError::fail(format!("{outcome:?}"))),
    }
}

// 256 cases each by default; PROPTEST_CASES=300000 runs a longer search.
proptest! {
    #![proptest_config(Config {
        failure_persistence: None,
        ..Config::default()
    })]

    #[test]
    fn generated_notes_keep_their_bytes(note in note()) {
        check(&note)?;
    }

    #[test]
    fn scrambled_notes_keep_their_bytes(note in scramble()) {
        check(&note)?;
    }
}
