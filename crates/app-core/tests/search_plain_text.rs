//! Seam 1: whatever you type into search is plain text. Quotes,
//! asterisks, parentheses, colons, hyphens, carets and the words AND, OR,
//! NOT and NEAR never act as full-text query syntax, and no query fails.

mod support;

use app_core::{SearchQuery, Workspace, WorkspaceService};
use proptest::{
    prelude::*,
    test_runner::{Config, TestRunner},
};
use support::Fixture;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// The notes every query runs against, with the text search reads from
/// each: its name, its properties and its body.
const NOTES: [(&str, &str, &str); 5] = [
    (
        "Risotto.md",
        "Add the saffron last. Stir and taste.\n",
        "Risotto Add the saffron last. Stir and taste.",
    ),
    (
        "Pepper.md",
        "Black pepper or white pepper, near the stove.\n",
        "Pepper Black pepper or white pepper, near the stove.",
    ),
    (
        "Notes/Café.md",
        "Un café noir, s'il vous plaît.\n",
        "Notes Café Un café noir, s'il vous plaît.",
    ),
    (
        "Syntax.md",
        "title: body (parens) \"quoted\" star* caret^ col:on a-b\n",
        "Syntax title: body (parens) \"quoted\" star* caret^ col:on a-b",
    ),
    (
        "Tagged.md",
        "---\nstatus: NEAR done\n---\nNothing else here\n",
        "Tagged NEAR done Nothing else here",
    ),
];

fn fixture() -> Fixture {
    let files: Vec<(&str, &[u8])> = NOTES
        .iter()
        .map(|(path, bytes, _)| (*path, bytes.as_bytes()))
        .collect();
    Fixture::new(&files)
}

fn open(fixture: &Fixture) -> Workspace {
    WorkspaceService::new(fixture.app_data.path())
        .open_folder(&fixture.root)
        .unwrap()
}

fn titles(workspace: &Workspace, text: &str) -> Vec<String> {
    let mut found: Vec<String> = workspace
        .search(&SearchQuery {
            text: text.to_owned(),
            ..SearchQuery::default()
        })
        .unwrap_or_else(|error| panic!("{text:?} failed: {error:?}"))
        .into_iter()
        .map(|hit| hit.title)
        .collect();
    found.sort();
    found
}

/// Lowercase, without accents.
fn fold(text: &str) -> String {
    text.nfd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The runs of letters and digits in `text`, folded.
fn words(text: &str) -> Vec<String> {
    fold(text)
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether a note could match `query` as plain text: every run of letters
/// and digits in it starts some word of the note.
fn could_match(note: &str, query: &str) -> bool {
    let note = words(note);
    words(query)
        .iter()
        .all(|wanted| note.iter().any(|word| word.starts_with(wanted.as_str())))
}

#[test]
fn query_syntax_reads_as_plain_words() {
    let fixture = fixture();
    let workspace = open(&fixture);

    // As syntax, each of these would find a note; as plain text, none does.
    assert!(titles(&workspace, "body:saffron").is_empty());
    assert!(titles(&workspace, "NOT pepper").is_empty());
    assert!(titles(&workspace, "pepper OR saffron").is_empty());
    assert!(titles(&workspace, "NEAR(pepper stove)").is_empty());
    assert!(titles(&workspace, "saffron NOT taste").is_empty());
    // Punctuation around a word leaves the word.
    for text in ["saffron*", "\"saffron", "(saffron)", "^add", "saffron:"] {
        assert_eq!(titles(&workspace, text), ["Risotto"], "{text}");
    }
    // AND, OR and NEAR are words like any other.
    assert_eq!(titles(&workspace, "saffron AND"), ["Risotto"]);
    assert_eq!(titles(&workspace, "pepper or"), ["Pepper"]);
    assert_eq!(titles(&workspace, "near done"), ["Tagged"]);
    // Hyphens and colons join words that sit together.
    assert_eq!(titles(&workspace, "a-b"), ["Syntax"]);
    assert_eq!(titles(&workspace, "col:on"), ["Syntax"]);
    // Punctuation alone, or a quote, finds nothing and fails nothing.
    for text in ["\"", "\"\"", "*", "()", ":", "^", "-", "\" * \"", "\0"] {
        assert!(titles(&workspace, text).is_empty(), "{text:?}");
    }
}

/// Pieces that full-text query syntax would read as operators, and words
/// from the notes, which make matches likely.
fn piece() -> impl Strategy<Value = String> {
    let syntax = prop::sample::select(vec![
        "\"", "\"\"", "*", "(", ")", ":", "-", "^", "+", "{", "}", "AND", "OR", "NOT", "NEAR",
        "NEAR(", "NEAR/2", "title:", "body:", "{title}", "-body:", "and", "or", "not", "near",
    ]);
    let note_word = prop::sample::select(vec![
        "saffron", "saff", "pepper", "pep", "cafe", "café", "noir", "stove", "done", "the",
        "parens", "quoted", "star", "caret", "col", "on", "a", "b", "taste", "nothing",
    ]);
    prop_oneof![
        syntax.prop_map(str::to_owned),
        note_word.prop_map(str::to_owned),
        "[a-zA-Z0-9éèàüç\"*():^+{}/-]{1,8}",
    ]
}

/// A query made of pieces, with or without spaces between them.
fn query() -> impl Strategy<Value = String> {
    prop::collection::vec((piece(), prop::bool::ANY), 1..7).prop_map(|pieces| {
        let mut text = String::new();
        for (piece, space) in pieces {
            text.push_str(&piece);
            if space {
                text.push(' ');
            }
        }
        text
    })
}

#[test]
fn every_query_returns_plain_text_matches_or_none() {
    let fixture = fixture();
    let workspace = open(&fixture);
    let mut runner = TestRunner::new(Config {
        cases: 512,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(&query(), |text| {
            let found = titles(&workspace, &text);
            for title in &found {
                let (_, _, note) = NOTES
                    .iter()
                    .find(|(path, _, _)| path.ends_with(&format!("{title}.md")))
                    .unwrap();
                prop_assert!(
                    could_match(note, &text),
                    "{text:?} found {title}, which doesn't hold every word"
                );
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn no_string_makes_search_fail() {
    let fixture = fixture();
    let workspace = open(&fixture);
    let mut runner = TestRunner::new(Config {
        cases: 512,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(&any::<String>(), |text| {
            let result = workspace.search(&SearchQuery {
                text: text.clone(),
                ..SearchQuery::default()
            });
            prop_assert!(result.is_ok(), "{text:?} failed: {result:?}");
            Ok(())
        })
        .unwrap();
    // A long query, too.
    let long = "saffron ".repeat(2_000);
    assert_eq!(titles(&workspace, &long), ["Risotto"]);
}
