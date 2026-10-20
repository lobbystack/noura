//! The theme follows the system appearance.

mod support;

use app_ui::theme::Palette;
use egui::Theme;
use support::{FakePicker, harness};

#[test]
fn the_window_follows_the_system_appearance() {
    let app_data = tempfile::tempdir().unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());

    for (system, palette) in [
        (Theme::Dark, Palette::dark()),
        (Theme::Light, Palette::light()),
        (Theme::Dark, Palette::dark()),
    ] {
        harness.input_mut().system_theme = Some(system);
        harness.run();

        assert_eq!(harness.ctx.theme(), system);
        let visuals = harness.ctx.global_style().visuals.clone();
        assert_eq!(visuals.panel_fill, palette.background);
        assert_eq!(visuals.window_fill, palette.popover);
        assert_eq!(visuals.widgets.inactive.fg_stroke.color, palette.foreground);
    }
}
