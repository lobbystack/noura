//! Seam 2: fenced code with a known language tag gets syntax highlighting,
//! other code blocks draw as plain monospace, and Markdown inside code
//! stays literal.

use editor_core::MdDoc;
use editor_core::constructs::ConstructKind;
use editor_core::highlight::CodeStyle;

/// Each highlighted piece of code in `text`, with its style.
fn highlights(text: &str) -> Vec<(&str, CodeStyle)> {
    let mut doc = MdDoc::new(text);
    let highlights = doc.code_highlights();
    assert_eq!(doc.text(), text);
    highlights
        .into_iter()
        .map(|(range, style)| (&text[range.0.0..range.1.0], style))
        .collect()
}

#[test]
fn fenced_code_with_a_known_language_is_highlighted() {
    let found = highlights("```rust\nfn main() {\n    let answer = 42; // reply\n}\n```\n");

    for expected in [
        ("fn", CodeStyle::Keyword),
        ("main", CodeStyle::Function),
        ("let", CodeStyle::Keyword),
        ("42", CodeStyle::Number),
        ("// reply", CodeStyle::Comment),
    ] {
        assert!(found.contains(&expected), "{expected:?} in {found:?}");
    }
}

#[test]
fn language_tags_take_their_common_names() {
    for tag in ["js", "javascript", "JS", "ts", "py", "sh", "yml"] {
        let text = format!("```{tag}\nkey: \"text\"\n```\n");
        let found: Vec<CodeStyle> = highlights(&text)
            .into_iter()
            .map(|(_, style)| style)
            .collect();
        assert!(found.contains(&CodeStyle::String), "{tag}: {found:?}");
    }
}

#[test]
fn other_code_blocks_draw_as_plain_text() {
    for text in [
        "```\nfn main() {}\n```\n",
        "```made-up-language\nfn main() {}\n```\n",
        "    fn main() {}\n",
        "`fn main() {}`\n",
    ] {
        assert_eq!(highlights(text), vec![], "{text:?}");
    }
}

#[test]
fn markdown_inside_code_stays_literal() {
    let text = "```markdown\n# Not a heading\n**not bold** [[not a link]] #not-a-tag\n```\n";

    let kinds: Vec<ConstructKind> = MdDoc::new(text)
        .constructs()
        .into_iter()
        .map(|construct| construct.kind)
        .collect();

    assert_eq!(kinds, vec![ConstructKind::CodeBlock]);
}
