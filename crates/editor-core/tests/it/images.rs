//! Seam 2: images draw inline from `![alt](path)` and from `![[image.png]]`
//! in the formats today's app shows, and `![[image.png|300]]` sets the
//! width. The embedding app finds and loads what each names.

use editor_core::MdDoc;
use editor_core::constructs::{Construct, ConstructKind};

fn images(text: &str) -> Vec<Construct> {
    let mut doc = MdDoc::new(text);
    let constructs = doc.constructs();
    assert_eq!(doc.text(), text, "drawing an image writes nothing");
    constructs
        .into_iter()
        .filter(|construct| matches!(construct.kind, ConstructKind::Image { .. }))
        .collect()
}

/// Each image: its source, what it names, and the width it asks for.
fn described(text: &str) -> Vec<(&str, String, Option<u32>)> {
    images(text)
        .into_iter()
        .map(|image| {
            let ConstructKind::Image { width } = image.kind else {
                unreachable!()
            };
            (
                &text[image.range.0.0..image.range.1.0],
                image.link.expect("an image names a target").target,
                width,
            )
        })
        .collect()
}

#[test]
fn markdown_images_and_image_embeds_draw_inline() {
    let text = "![Alt](pic.png \"A title\")\n\n![[photo.jpg|300]] and ![[Diagram.SVG]]\n\n![[Other note]]\n";

    assert_eq!(
        described(text),
        vec![
            ("![Alt](pic.png \"A title\")", "pic.png".to_string(), None),
            ("![[photo.jpg|300]]", "photo.jpg".to_string(), Some(300)),
            ("![[Diagram.SVG]]", "Diagram.SVG".to_string(), None),
        ]
    );
}

#[test]
fn embeds_name_images_in_the_formats_today_s_app_shows() {
    for name in [
        "a.png", "a.jpg", "a.jpeg", "a.gif", "a.webp", "a.svg", "a.bmp", "a.avif",
    ] {
        let text = format!("![[{name}]]\n");
        assert_eq!(images(&text).len(), 1, "{name}");
    }
    for name in ["a.txt", "a.png.md", "a.pdf.md", "a", ".pdf"] {
        let text = format!("![[{name}]]\n");
        assert_eq!(images(&text).len(), 0, "{name}");
    }
}

#[test]
fn an_embed_of_a_pdf_draws_inline_as_the_page_its_fragment_names() {
    let text = "![[lecture.pdf#page=7]]\n\nInline ![[Scans/Old.PDF|400]] end\n";

    assert_eq!(
        described(text),
        vec![
            (
                "![[lecture.pdf#page=7]]",
                "lecture.pdf#page=7".to_string(),
                None
            ),
            (
                "![[Scans/Old.PDF|400]]",
                "Scans/Old.PDF".to_string(),
                Some(400)
            ),
        ]
    );
}

#[test]
fn a_width_needs_digits() {
    let text = "![[a.png|300x200]] ![[b.png|wide]] ![[c.png|0]]\n";

    let widths: Vec<Option<u32>> = described(text)
        .into_iter()
        .map(|(_, _, width)| width)
        .collect();

    assert_eq!(widths, vec![Some(300), None, None]);
}
