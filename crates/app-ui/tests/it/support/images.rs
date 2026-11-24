//! Images for the image tab's and the notes' tests: PNGs saved at a
//! density, SVGs of a declared size, a window at a screen's density, and
//! where an image draws and the textures egui holds for it.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use app_core::{App, Platform};
use app_ui::NouraUi;
use egui::{Rect, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable};

use super::{FakePicker, FakeSystem, create_workspace, settle};

pub type Ui = Harness<'static, NouraUi>;

/// The CRC that ends a PNG chunk.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A PNG, `width` by `height` pixels of gray stripes, saved at `dpi` dots
/// per inch when given, as a Retina screenshot saves 144.
pub fn png_at(width: u32, height: u32, dpi: Option<u32>) -> Vec<u8> {
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_fn(width, height, |x, _| {
        let level = if x.is_multiple_of(2) { 40 } else { 220 };
        image::Rgba([level, level, level, 255])
    })
    .write_to(&mut png, image::ImageFormat::Png)
    .unwrap();
    let png = png.into_inner();
    let Some(dpi) = dpi else {
        return png;
    };
    // A pHYs chunk after the 33 bytes of signature and header.
    let per_meter = (f64::from(dpi) / 0.0254).round() as u32;
    let mut chunk = b"pHYs".to_vec();
    chunk.extend(per_meter.to_be_bytes());
    chunk.extend(per_meter.to_be_bytes());
    chunk.push(1);
    let mut out = png[..33].to_vec();
    out.extend(9u32.to_be_bytes());
    out.extend(&chunk);
    out.extend(crc32(&chunk).to_be_bytes());
    out.extend(&png[33..]);
    out
}

/// An SVG that declares itself `width` by `height`.
pub fn svg(width: u32, height: u32) -> Vec<u8> {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><rect width="{width}" height="{height}" fill="teal"/><line x1="0" y1="0" x2="{width}" y2="{height}" stroke="black" stroke-width="0.5"/></svg>"#
    )
    .into_bytes()
}

/// A window `size` points large at `pixels_per_point`, on a workspace
/// holding `files`.
pub fn window(
    app_data: &Path,
    folder: &Path,
    files: &[(&str, &[u8])],
    size: [f32; 2],
    pixels_per_point: f32,
) -> Ui {
    create_workspace(app_data, folder);
    for (name, bytes) in files {
        std::fs::write(folder.join(name), bytes).unwrap();
    }
    let app = App::launch_with(app_data, FakeSystem::new(Platform::current()));
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    let mut harness = super::builder()
        .with_size(size)
        .with_pixels_per_point(pixels_per_point)
        .with_max_steps(64)
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui);
    settle(&mut harness);
    harness
}

/// Run frames, waiting in real time, until `done` holds.
pub fn wait_until(harness: &mut Ui, what: &str, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(harness) {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
}

/// Where the image labelled `name` draws, once it does.
pub fn drawn(harness: &Ui, name: &str) -> Option<Rect> {
    harness
        .query_by_role_and_label(Role::Image, name)
        .map(|node| node.rect())
}

/// The sizes of the textures egui holds whose name holds `needle`.
pub fn textures(harness: &Ui, needle: &str) -> Vec<[usize; 2]> {
    let manager = harness.ctx.tex_manager();
    let manager = manager.read();
    manager
        .allocated()
        .filter(|(_, meta)| meta.name.contains(needle))
        .map(|(_, meta)| meta.size)
        .collect()
}
