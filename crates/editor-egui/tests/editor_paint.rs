//! The editor paints its text through egui-wgpu, the renderer eframe uses,
//! with the glyphon paint callback. Renders offscreen with `egui_kittest`
//! and skips when the machine has no wgpu adapter.

use std::sync::{Arc, Mutex};

use editor_egui::{Editor, MdConfig};
use egui_kittest::Harness;
use egui_kittest::wgpu::WgpuTestRenderer;
use glyphon::FontSystem;

/// A fresh wgpu render state, or `None` when no adapter is available.
fn render_state() -> Option<egui_wgpu::RenderState> {
    let setup = egui_kittest::wgpu::default_wgpu_setup();
    let instance = pollster::block_on(setup.new_instance());
    pollster::block_on(egui_wgpu::RenderState::create(
        &egui_wgpu::WgpuConfiguration {
            wgpu_setup: setup,
            ..Default::default()
        },
        &instance,
        None,
        egui_wgpu::RendererOptions::PREDICTABLE,
    ))
    .ok()
}

#[test]
fn the_editor_paints_text_through_egui_wgpu() {
    let Some(render_state) = render_state() else {
        eprintln!("skipped: no wgpu adapter on this machine");
        return;
    };
    let font_system = Arc::new(Mutex::new(FontSystem::new_with_locale_and_db(
        "en-US".into(),
        editor_egui::fonts::default_font_database(),
    )));
    editor_egui::register_render_callback_resources(
        &render_state.device,
        &render_state.queue,
        render_state.target_format,
        &mut render_state.renderer.write(),
        Arc::clone(&font_system),
        1,
    );

    let mut harness = Harness::builder()
        .with_size(egui::vec2(400.0, 160.0))
        .renderer(WgpuTestRenderer::from_render_state(render_state))
        .with_context_setup(move |ctx| editor_egui::install_font_system(ctx, font_system))
        .build_ui_state(
            |ui, editor: &mut Option<Editor>| {
                editor
                    .get_or_insert_with(|| {
                        Editor::new(ui.ctx(), "# Heading\n\nSome text.\n", MdConfig::default())
                    })
                    .show(ui);
            },
            None,
        );
    harness.run_steps(2);
    let image = harness.render().expect("the frame renders");

    // Inside the heading's span, most pixels are the editor's background
    // and the rest are glyphs. Glyphon draws all text, so glyph pixels came
    // through the paint callback.
    let editor = harness.state().as_ref().unwrap();
    let heading = editor
        .painted_spans()
        .into_iter()
        .find(|span| span.glyphs && span.source_range.0.0 >= 2)
        .expect("the heading drew text")
        .rect;
    let ppp = harness.ctx.pixels_per_point();
    let pixels: Vec<[u8; 4]> = (heading.min.y as u32..heading.max.y as u32)
        .flat_map(|y| (heading.min.x as u32..heading.max.x as u32).map(move |x| (x, y)))
        .map(|(x, y)| ((x as f32 * ppp) as u32, (y as f32 * ppp) as u32))
        .filter(|&(x, y)| x < image.width() && y < image.height())
        .map(|(x, y)| image.get_pixel(x, y).0)
        .collect();
    let mut counts = std::collections::HashMap::new();
    for pixel in &pixels {
        *counts.entry(*pixel).or_insert(0) += 1;
    }
    let background = counts.into_iter().max_by_key(|&(_, n)| n).unwrap().0;
    let text_pixels = pixels
        .iter()
        .filter(|&&pixel| distance(pixel, background) > 120)
        .count();
    assert!(
        text_pixels > 20,
        "the heading painted {text_pixels} text pixels"
    );
}

/// How far apart two pixels sit, channel by channel.
fn distance(a: [u8; 4], b: [u8; 4]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(y) as u32)
        .sum::<u32>()
}

/// An offscreen harness on `text` in `theme`'s appearance, with `frames`
/// frames run, or `None` when no adapter is available.
fn painted(
    text: &'static str,
    theme: egui::Theme,
    frames: usize,
) -> Option<Harness<'static, Option<Editor>>> {
    let render_state = render_state()?;
    let font_system = Arc::new(Mutex::new(FontSystem::new_with_locale_and_db(
        "en-US".into(),
        editor_egui::fonts::default_font_database(),
    )));
    editor_egui::register_render_callback_resources(
        &render_state.device,
        &render_state.queue,
        render_state.target_format,
        &mut render_state.renderer.write(),
        Arc::clone(&font_system),
        1,
    );
    let mut harness = Harness::builder()
        .with_size(egui::vec2(500.0, 300.0))
        .with_theme(theme)
        .renderer(WgpuTestRenderer::from_render_state(render_state))
        .with_context_setup(move |ctx| editor_egui::install_font_system(ctx, font_system))
        .build_ui_state(
            move |ui, editor: &mut Option<Editor>| {
                editor
                    .get_or_insert_with(|| Editor::new(ui.ctx(), text, MdConfig::default()))
                    .show(ui);
            },
            None,
        );
    for _ in 0..frames {
        harness.step();
    }
    Some(harness)
}

/// The pixels `pixel` reads inside `rect`, in points, on an image of
/// `size` pixels.
fn pixels_in(
    harness: &Harness<'static, Option<Editor>>,
    size: (u32, u32),
    pixel: impl Fn(u32, u32) -> [u8; 4],
    rect: egui::Rect,
) -> Vec<[u8; 4]> {
    let ppp = harness.ctx.pixels_per_point();
    let (left, top) = ((rect.min.x * ppp) as u32, (rect.min.y * ppp) as u32);
    let (right, bottom) = ((rect.max.x * ppp) as u32, (rect.max.y * ppp) as u32);
    (top..bottom.min(size.1))
        .flat_map(|y| (left..right.min(size.0)).map(move |x| (x, y)))
        .map(|(x, y)| pixel(x, y))
        .collect()
}

/// Highlighted code reaches the screen in the colors the editor chose.
#[test]
fn highlighted_code_reaches_the_screen_in_its_colors() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let Some(mut harness) = painted("```rust\nfn main() {}\n```\n\nafter\n", theme, 3) else {
            eprintln!("skipped: no wgpu adapter on this machine");
            return;
        };
        let image = harness.render().expect("the frame renders");
        let spans = harness.state().as_ref().unwrap().painted_spans();
        for token in ["fn", "main"] {
            let span = spans
                .iter()
                .find(|span| span.glyphs && span.text == token)
                .unwrap_or_else(|| panic!("{token} drew nothing"));
            let color = span.color.expect("the token drew glyphs").to_array();
            let size = (image.width(), image.height());
            let matching = pixels_in(&harness, size, |x, y| image.get_pixel(x, y).0, span.rect)
                .into_iter()
                .filter(|pixel| distance(*pixel, color) < 60)
                .count();
            assert!(matching > 2, "{token}: {matching} pixels of {color:?}");
        }
    }
}

/// A checked box shows light mark pixels on its filled square, and an open
/// box shows none.
#[test]
fn a_checked_box_paints_its_mark_in_light_and_dark() {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let Some(mut harness) = painted("top\n\n- [x] done\n- [ ] open\n", theme, 30) else {
            eprintln!("skipped: no wgpu adapter on this machine");
            return;
        };
        let image = harness.render().expect("the frame renders");
        let spans = harness.state().as_ref().unwrap().painted_spans();
        let box_left_of = |text: &str| {
            let span = spans
                .iter()
                .find(|span| span.glyphs && span.text == text)
                .unwrap_or_else(|| panic!("{text} drew nothing"));
            egui::Rect::from_center_size(
                egui::pos2(
                    span.rect.left() - span.rect.height() * 0.6,
                    span.rect.center().y,
                ),
                egui::Vec2::splat(span.rect.height() * 0.4),
            )
        };

        let white = [255, 255, 255, 255];
        let accent = editor_egui::palette::Theme::default(match theme {
            egui::Theme::Light => editor_egui::palette::Mode::Light,
            egui::Theme::Dark => editor_egui::palette::Mode::Dark,
        })
        .accent()
        .to_array();
        let count = |pixels: &[[u8; 4]], color: [u8; 4]| {
            pixels
                .iter()
                .filter(|pixel| distance(**pixel, color) < 60)
                .count()
        };
        let size = (image.width(), image.height());
        let read = |x, y| image.get_pixel(x, y).0;
        let checked = pixels_in(&harness, size, read, box_left_of("done"));
        assert!(count(&checked, white) > 3, "{theme:?}: no mark");
        assert!(count(&checked, accent) > 10, "{theme:?}: no fill");
        let open = pixels_in(&harness, size, read, box_left_of("open"));
        assert_eq!(count(&open, accent), 0, "{theme:?}: an open box is filled");
    }
}
