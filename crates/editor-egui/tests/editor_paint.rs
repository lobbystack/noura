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
    let distance = |a: [u8; 4], b: [u8; 4]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| x.abs_diff(y) as u32)
            .sum::<u32>()
    };
    let text_pixels = pixels
        .iter()
        .filter(|&&pixel| distance(pixel, background) > 120)
        .count();
    assert!(
        text_pixels > 20,
        "the heading painted {text_pixels} text pixels"
    );
}
