//! The image tab through AccessKit, as Preview shows an image: at the
//! density its file records and sharp at the screen's, fit to the tab
//! until you zoom, from the pixels it covers on the screen, and zoomed by
//! a pinch at the pointer or by Zoom In, Zoom Out and Actual Size, which
//! zoom the interface on any other tab.

use crate::support::images::{Ui, drawn, png_at, svg, textures, wait_until, window};
use app_core::OpenAs;
use egui::{Event, Key, Modifiers, Vec2, vec2};

const CMD: Modifiers = Modifiers::COMMAND;

/// The sizes of the textures of the image tab's images with `extension`.
fn tab_textures(harness: &Ui, extension: &str) -> Vec<[usize; 2]> {
    let manager = harness.ctx.tex_manager();
    let manager = manager.read();
    manager
        .allocated()
        .filter(|(_, meta)| {
            meta.name.contains("noura-viewer/") && meta.name.contains(&format!(".{extension}"))
        })
        .map(|(_, meta)| meta.size)
        .collect()
}

/// Open `name` in a tab of its own and wait until it draws at `size`
/// points, from a texture of `pixels`.
fn open_image(harness: &mut Ui, name: &str, size: Vec2, pixels: [usize; 2]) {
    harness.state_mut().app_mut().open_file(name, OpenAs::Kept);
    let extension = name.rsplit_once('.').unwrap().1;
    wait_until(harness, &format!("{name} draws at {size:?}"), |harness| {
        drawn(harness, name).is_some_and(|rect| (rect.size() - size).length() < 0.5)
            && tab_textures(harness, extension).contains(&pixels)
    });
}

#[test]
fn an_image_shows_at_the_density_its_file_records_and_sharp_on_a_retina_screen() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = window(
        app_data.path(),
        &parent.path().join("Garden"),
        &[
            ("retina.png", &png_at(800, 500, Some(144))),
            ("plain.png", &png_at(300, 200, None)),
            ("diagram.svg", &svg(400, 300)),
        ],
        [1200.0, 800.0],
        2.0,
    );

    // A Retina screenshot shows at half its pixels, each on a pixel of
    // the screen.
    open_image(&mut harness, "retina.png", vec2(400.0, 250.0), [800, 500]);
    // An image with no density shows at a point a pixel.
    open_image(&mut harness, "plain.png", vec2(300.0, 200.0), [300, 200]);
    // An SVG shows at its declared size, drawn at the screen's pixels.
    open_image(&mut harness, "diagram.svg", vec2(400.0, 300.0), [800, 600]);
}

#[test]
fn an_image_drawn_smaller_uploads_only_the_pixels_it_covers() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = window(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("wide.png", &png_at(1600, 400, None))],
        [1000.0, 700.0],
        1.0,
    );
    harness
        .state_mut()
        .app_mut()
        .open_file("wide.png", OpenAs::Kept);
    wait_until(&mut harness, "the image fits the tab", |harness| {
        drawn(harness, "wide.png").is_some_and(|rect| rect.width() < 1000.0)
    });
    let rect = drawn(&harness, "wide.png").unwrap();
    let pixels = [
        rect.width().round() as usize,
        rect.height().round() as usize,
    ];
    // The GPU would skip most of the stripes of a texture of all 1,600.
    wait_until(
        &mut harness,
        "the image uploads at its drawn size",
        |harness| textures(harness, "noura-viewer") == [pixels],
    );
}

#[test]
fn the_image_tab_fits_then_zooms_and_scrolls_like_preview() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = window(
        app_data.path(),
        &parent.path().join("Garden"),
        &[
            ("map.png", &png_at(1600, 800, None)),
            ("Plan.md", b"# Plan\n"),
        ],
        [1000.0, 700.0],
        1.0,
    );
    harness
        .state_mut()
        .app_mut()
        .open_file("map.png", OpenAs::Kept);
    wait_until(&mut harness, "the image draws", |harness| {
        drawn(harness, "map.png").is_some()
    });
    let tab = harness.state().app().active_tab().unwrap().id();
    let fit = drawn(&harness, "map.png").unwrap();
    // It opens fit to the tab, as wide as the tab and centered down it.
    assert!(fit.width() < 800.0 && fit.top() > 50.0, "{fit:?}");
    let middle = fit.center();

    // Command-= zooms the image, not the interface.
    harness.key_press_modifiers(CMD, Key::Equals);
    harness.run_steps(4);
    assert_eq!(harness.ctx.zoom_factor(), 1.0);
    let zoomed = drawn(&harness, "map.png").unwrap();
    assert!((zoomed.width() - 800.0).abs() < 1.0, "{zoomed:?}");

    // Command-0 shows it at its own size, larger than the tab, which
    // scrolls to show the rest.
    harness.key_press_modifiers(CMD, Key::Num0);
    harness.run_steps(4);
    let actual = drawn(&harness, "map.png").unwrap();
    assert!((actual.width() - 1600.0).abs() < 1.0, "{actual:?}");
    let before = actual.left();
    harness.event(Event::PointerMoved(middle));
    harness.event(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(-200.0, 0.0),
        modifiers: Modifiers::NONE,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    harness.run_steps(4);
    let scrolled = drawn(&harness, "map.png").unwrap();
    assert!(scrolled.left() < before - 100.0, "{before} -> {scrolled:?}");

    // Command-minus steps back down.
    harness.key_press_modifiers(CMD, Key::Minus);
    harness.run_steps(4);
    let smaller = drawn(&harness, "map.png").unwrap();
    assert!((smaller.width() - 1440.0).abs() < 1.0, "{smaller:?}");

    // A pinch zooms at the pointer: the image's point under it stays.
    let rect = drawn(&harness, "map.png").unwrap();
    let pointer = middle + vec2(120.0, 40.0);
    let point = (pointer - rect.min) / rect.width();
    harness.event(Event::PointerMoved(pointer));
    harness.step();
    harness.event(Event::Zoom(1.5));
    harness.run_steps(4);
    let pinched = drawn(&harness, "map.png").unwrap();
    assert!((pinched.width() / rect.width() - 1.5).abs() < 0.01);
    let under = pinched.min + point * pinched.width();
    assert!((under - pointer).length() < 1.5, "{under:?} vs {pointer:?}");

    // On any other tab, Command-= zooms the interface.
    harness
        .state_mut()
        .app_mut()
        .open_file("Plan.md", OpenAs::Kept);
    harness.run_steps(4);
    harness.key_press_modifiers(CMD, Key::Equals);
    harness.run_steps(4);
    assert_eq!(harness.state().app().preferences().zoom, 110);
    // The image keeps its zoom, and its size on the screen: 2,160 pixels
    // across, 1,964 of the interface's larger points.
    harness.state_mut().app_mut().activate_tab(tab);
    harness.run_steps(4);
    let kept = drawn(&harness, "map.png").unwrap();
    let pixels = kept.width() * harness.ctx.pixels_per_point();
    assert!((pixels - 2160.0).abs() < 1.5, "{kept:?} is {pixels} pixels");
}
