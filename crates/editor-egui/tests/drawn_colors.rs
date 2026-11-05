//! The colors the editor draws in light and dark: each kind of highlighted
//! code stays readable on what it draws over, and a checked box shows its
//! mark against its fill.

use editor_egui::palette::{Mode, Theme, ThemeExt as _, contrast_ratio};
use editor_egui::{Editor, MdConfig};
use egui::{
    Color32, Shape,
    epaint::{ClippedShape, ColorMode},
};
use egui_kittest::Harness;

const CODE: &str = "intro\n\n```rust\n// a comment\nstruct Point;\nfn main() {\n    \
                    let total = 42;\n    let name = \"text\";\n}\n```\n";

/// Each piece of `CODE` and the kind of token it draws as.
const TOKENS: [&str; 6] = ["comment", "struct", "main", "Point", "42", "text"];

/// A harness showing an editor on `text` in `theme`'s appearance, with the
/// theme set when `set` holds, as an app that picks its own does.
fn harness(text: &'static str, mode: Mode, set: Option<Theme>) -> Harness<'static, Option<Editor>> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_theme(match mode {
            Mode::Light => egui::Theme::Light,
            Mode::Dark => egui::Theme::Dark,
        })
        .with_context_setup(move |ctx| {
            editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
            if let Some(theme) = set {
                ctx.set_lb_theme(theme);
            }
        })
        .build_ui_state(
            move |ui, editor: &mut Option<Editor>| {
                editor
                    .get_or_insert_with(|| Editor::new(ui.ctx(), text, MdConfig::default()))
                    .show(ui);
            },
            None,
        );
    harness.run_steps(3);
    harness
}

fn editor<'a>(harness: &'a Harness<'static, Option<Editor>>) -> &'a Editor {
    harness.state().as_ref().expect("the first frame drew it")
}

/// The color each of `TOKENS` drew in.
fn token_colors(harness: &Harness<'static, Option<Editor>>) -> Vec<(&'static str, Color32)> {
    let spans = editor(harness).painted_spans();
    TOKENS
        .into_iter()
        .map(|token| {
            let span = spans
                .iter()
                .find(|span| span.glyphs && span.text.contains(token))
                .unwrap_or_else(|| panic!("{token} drew nothing: {spans:#?}"));
            (token, span.color.expect("the token drew glyphs"))
        })
        .collect()
}

#[test]
fn highlighted_code_draws_a_color_for_each_kind_of_token() {
    for mode in [Mode::Light, Mode::Dark] {
        let harness = harness(CODE, mode, None);
        let colors = token_colors(&harness);
        let plain = editor(&harness)
            .painted_spans()
            .into_iter()
            .find(|span| span.text.contains("intro"))
            .expect("the paragraph drew")
            .color
            .expect("the paragraph drew glyphs");

        for (token, color) in &colors {
            assert_ne!(*color, plain, "{token} draws as plain text");
        }
        let mut kinds: Vec<[u8; 4]> = colors
            .iter()
            .filter(|(token, _)| *token != "Point")
            .map(|(_, color)| color.to_array())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), 5, "{colors:?}");
    }
}

/// Code draws over the note's own background, so its colors must read on
/// that, at the 4.5:1 of normal text.
#[test]
fn every_code_color_reads_on_the_note_s_background() {
    for mode in [Mode::Light, Mode::Dark] {
        let harness = harness(CODE, mode, None);
        let background = harness.ctx.global_style().visuals.panel_fill;
        for (token, color) in token_colors(&harness) {
            assert!(
                contrast_ratio(color, background).passes_normal_text(),
                "{token} in {color:?} on {background:?}"
            );
        }
    }
}

/// A theme with code colors of its own fills the block, so they must read
/// on that fill.
#[test]
fn every_code_color_of_a_code_theme_reads_on_the_code_block() {
    for mode in [Mode::Light, Mode::Dark] {
        let theme = Theme::apple(mode);
        let background = theme.code_variant().expect("a code theme").background;
        let harness = harness(CODE, mode, Some(theme));
        for (token, color) in token_colors(&harness) {
            assert!(
                contrast_ratio(color, background).passes_normal_text(),
                "{token} in {color:?} on {background:?}"
            );
        }
    }
}

/// Every shape in `shapes`, flattened.
fn flat(shapes: &[ClippedShape]) -> Vec<Shape> {
    fn add(shape: &Shape, found: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|shape| add(shape, found)),
            shape => found.push(shape.clone()),
        }
    }
    let mut found = Vec::new();
    for clipped in shapes {
        add(&clipped.shape, &mut found);
    }
    found
}

/// The check mark's three points and color, and the fill drawn last under
/// them, from the frame's shapes.
fn check_mark(shapes: &[Shape]) -> Option<(Color32, Color32)> {
    shapes.iter().enumerate().find_map(|(index, shape)| {
        let Shape::Path(path) = shape else {
            return None;
        };
        if path.points.len() != 3 || path.closed {
            return None;
        }
        let fill = shapes[..index].iter().rev().find_map(|shape| match shape {
            Shape::Rect(rect)
                if path.points.iter().all(|point| rect.rect.contains(*point))
                    && rect.fill.a() == 255 =>
            {
                Some(rect.fill)
            }
            _ => None,
        })?;
        match path.stroke.color {
            ColorMode::Solid(mark) => Some((mark, fill)),
            ColorMode::UV(_) => None,
        }
    })
}

/// Sum of the red and green distance, the legacy check of a mark against
/// its fill.
fn distance(a: Color32, b: Color32) -> u32 {
    u32::from(a.r().abs_diff(b.r())) + u32::from(a.g().abs_diff(b.g()))
}

#[test]
fn a_checked_box_shows_its_mark_against_its_fill() {
    for mode in [Mode::Light, Mode::Dark] {
        let mut harness = harness("first line\n\n- [x] done\n- [ ] open\n", mode, None);
        // The box's fill and mark animate in.
        for _ in 0..30 {
            harness.step();
        }
        let shapes = flat(&harness.output().shapes);
        let (mark, fill) = check_mark(&shapes).expect("the checked box drew its mark");

        assert_eq!(mark.a(), 255, "the mark shows at full strength");
        assert!(distance(mark, fill) > 200, "{mark:?} on {fill:?}");
    }
}

#[test]
fn an_open_box_draws_no_visible_mark() {
    for mode in [Mode::Light, Mode::Dark] {
        let mut harness = harness("first line\n\n- [ ] open\n", mode, None);
        for _ in 0..30 {
            harness.step();
        }
        let shapes = flat(&harness.output().shapes);
        assert!(
            shapes.iter().all(|shape| !matches!(
                shape,
                Shape::Path(path)
                    if path.points.len() == 3
                        && !path.closed
                        && !matches!(path.stroke.color, ColorMode::Solid(color) if color.a() == 0)
            )),
            "an open box drew a mark"
        );
    }
}
