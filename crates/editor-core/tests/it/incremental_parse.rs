//! Seam 2: an edit reparses only the top-level blocks it reaches, and the
//! tree it leaves is the one a full parse of the new text gives.

use editor_core::ast::Reparsed;
use editor_core::comrak::nodes::AstNode;
use editor_core::{Ast, MdDoc};
use proptest::prelude::*;

/// Every node of the tree, one line each: its depth, value and position.
fn dump(ast: &Ast) -> Vec<String> {
    fn walk<'a>(node: &'a AstNode<'a>, depth: usize, out: &mut Vec<String>) {
        let data = node.data.borrow();
        out.push(format!("{depth} {:?} {}", data.value, data.sourcepos));
        drop(data);
        for child in node.children() {
            walk(child, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    ast.with_root(|root| walk(root, 0, &mut out));
    out
}

/// A full parse of `text`, as the editor parses it.
fn parse(text: &str) -> Ast {
    Ast::parse(format!("{text}\n"), &MdDoc::comrak_options(), 0)
}

/// `before`'s parse, updated to `after`.
fn update(before: &str, after: &str) -> (Ast, Reparsed) {
    let mut ast = parse(before);
    let reparsed = ast.update(format!("{after}\n"), &MdDoc::comrak_options(), 1);
    (ast, reparsed)
}

/// Asserts that updating `before`'s parse to `after` gives a full parse of
/// `after`, and returns what it reparsed.
fn assert_matches_full_parse(before: &str, after: &str) -> Reparsed {
    let (ast, reparsed) = update(before, after);
    let expected = dump(&parse(after));
    let got = dump(&ast);
    if got != expected {
        let first = got
            .iter()
            .zip(&expected)
            .position(|(got, expected)| got != expected);
        panic!(
            "{before:?} -> {after:?}\nfirst difference at node {first:?}\ngot:      {:?}\nexpected: {:?}",
            first.map(|at| &got[at]),
            first.map(|at| &expected[at]),
        );
    }
    assert_eq!(ast.source(), format!("{after}\n"));
    reparsed
}

/// A note of `sections` sections of mixed blocks.
fn long_note(sections: usize) -> String {
    let mut note = String::from("---\ntags: [team]\n---\n\n# Notebook\n\n");
    for section in 0..sections {
        note.push_str(&format!(
            "## Section {section}\n\nA paragraph with *emphasis*, a [link](https://example.com/{section}) \
             and a [[Wiki {section}]].\nIt runs on a second line.\n\n\
             - one\n- two\n  - nested {section}\n- [ ] a task\n\n\
             > A quote\n> on two lines.\n\n\
             | a | b |\n|---|---|\n| {section} | x |\n\n\
             ```rust\nfn f() {{}}\n```\n\n"
        ));
    }
    note
}

#[test]
fn typing_in_a_paragraph_reparses_only_the_blocks_around_it() {
    let before = long_note(40);
    let at = before.find("It runs on a second line.").unwrap() + 3;
    let mut after = before.clone();
    after.insert(at, 'x');

    let reparsed = assert_matches_full_parse(&before, &after);

    assert!(reparsed.blocks.len() <= 3, "{reparsed:?}");
    assert!(reparsed.old_lines.len() < 20, "{reparsed:?}");
    assert_eq!(reparsed.old_lines, reparsed.new_lines);
}

#[test]
fn line_changes_move_the_blocks_after_them() {
    let before = long_note(30);
    let middle = before.find("## Section 15").unwrap();
    let para = before[middle..].find("A paragraph").unwrap() + middle;

    // Return splits a paragraph; Backspace joins two lines.
    let mut split = before.clone();
    split.insert_str(para + 11, "\n\n");
    assert_matches_full_parse(&before, &split);
    assert_matches_full_parse(&split, &before);

    // Deleting the blank line between two blocks merges them.
    let quote = before[middle..].find("> A quote").unwrap() + middle;
    let mut merged = before.clone();
    merged.remove(quote - 1);
    assert_matches_full_parse(&before, &merged);

    // A pasted list, and a selection deleted across blocks.
    let mut pasted = before.clone();
    pasted.insert_str(para, "- pasted\n- list\n\n");
    assert_matches_full_parse(&before, &pasted);
    let end = before[middle..].find("```rust").unwrap() + middle;
    let mut cut = before.clone();
    cut.replace_range(para + 5..end, "");
    assert_matches_full_parse(&before, &cut);
}

#[test]
fn edits_at_the_start_and_the_end_of_the_note() {
    let before = long_note(5);
    assert_matches_full_parse(&before, &before.replacen("tags: [team]", "tags: [all]", 1));
    assert_matches_full_parse(&before, &format!("{before}The end."));
    assert_matches_full_parse(&before, &before[..before.len() - 5]);
    assert_matches_full_parse(&before, "");
    assert_matches_full_parse("", &before);
    assert_matches_full_parse("\u{feff}# A\n\nb\n", "\u{feff}# Ab\n\nb\n");
}

#[test]
fn crlf_and_lone_cr_line_endings() {
    let before = long_note(8).replace('\n', "\r\n");
    let at = before.find("It runs").unwrap();
    let mut after = before.clone();
    after.insert_str(at, "More.\r\n\r\n");
    assert_matches_full_parse(&before, &after);
    assert_matches_full_parse("a\rb\r\rc\r", "a\rb\r\rcd\r\r# e\r");
}

/// A note of blocks that commonly sit side by side, with or without blank
/// lines between them.
fn note() -> impl Strategy<Value = String> {
    let block = prop_oneof![
        "[a-z ]{1,30}",
        "#{1,3} [a-z]{1,10}",
        "- [a-z]{1,8}\n  - [a-z]{1,8}\n- [a-z]{1,8}",
        "> [a-z ]{1,12}\n[a-z ]{1,12}",
        "1\\. [a-z]{1,8}\n2\\. [a-z]{1,8}",
        "\\*\\*[a-z]{1,6}\\*\\* [a-z]{1,6}",
        "\\| a \\| b \\|\n\\|---\\|---\\|\n\\| [a-z]{1,4} \\| c \\|",
        "```\n[a-z]{1,8}\n```",
        "<!-- [a-z]{1,8} -->",
        "    [a-z]{1,8}",
        "---",
        "- \\[ \\] [a-z]{1,8}\n- \\[x\\] [a-z]{1,8}",
        "> \\[!note\\] [a-z]{1,8}\n> [a-z]{1,8}",
    ];
    prop::collection::vec((block, 0..3usize), 0..12).prop_map(|blocks| {
        blocks
            .into_iter()
            .map(|(block, blank)| format!("{block}\n{}", "\n".repeat(blank)))
            .collect()
    })
}

/// An edit that types, deletes or breaks a line.
fn edit() -> impl Strategy<Value = (prop::sample::Index, usize, String)> {
    (
        any::<prop::sample::Index>(),
        0..6usize,
        prop_oneof![
            "[a-z ]{0,4}",
            Just("\n".to_owned()),
            Just("\n\n".to_owned()),
            Just("- ".to_owned()),
            Just("`".to_owned()),
        ],
    )
}

/// Whether a construct in `text` reaches across blocks: front matter, an
/// unclosed code fence or HTML comment. Edits around one can change blocks
/// beyond the reparsed run, and the background full parse corrects them.
fn reaches_across_blocks(text: &str) -> bool {
    text.starts_with("---")
        || text.matches("```").count() % 2 == 1
        || text.matches("<!--").count() != text.matches("-->").count()
}

proptest! {
    #![proptest_config(ProptestConfig {
        max_global_rejects: 100_000,
        ..ProptestConfig::default()
    })]
    #[test]
    fn an_updated_parse_matches_a_full_parse(note in note(), (at, cut, typed) in edit()) {
        let at = at.index(note.len() + 1);
        let end = (at + cut).min(note.len());
        let mut after = note.clone();
        after.replace_range(at..end, &typed);
        prop_assume!(!reaches_across_blocks(&note) && !reaches_across_blocks(&after));
        assert_matches_full_parse(&note, &after);
    }
}
