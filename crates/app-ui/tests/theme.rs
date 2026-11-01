//! The theme follows the system appearance.

mod support;

use app_ui::theme::Palette;
use editor_egui::palette::contrast_ratio;
use editor_egui::{Editor, MdConfig};
use egui::Theme;
use egui_kittest::Harness;
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

/// Tailwind's scale, as the legacy app uses it: 12, 14 and 16 px text, and
/// a 4 px spacing unit.
#[test]
fn the_type_scale_and_spacing_keep_the_legacy_values() {
    let app_data = tempfile::tempdir().unwrap();
    let harness = harness(app_data.path(), FakePicker::default());
    let style = harness.ctx.global_style();

    let size = |text: egui::TextStyle| style.text_styles[&text].size;
    assert_eq!(size(egui::TextStyle::Small), 12.0);
    assert_eq!(size(egui::TextStyle::Body), 14.0);
    assert_eq!(size(egui::TextStyle::Button), 14.0);
    assert_eq!(size(egui::TextStyle::Heading), 16.0);

    let spacing = &style.spacing;
    assert_eq!(spacing.item_spacing, egui::vec2(8.0, 8.0));
    assert_eq!(spacing.button_padding, egui::vec2(12.0, 6.0));
    assert_eq!(spacing.interact_size, egui::vec2(32.0, 32.0));
    assert_eq!(spacing.window_margin, egui::Margin::same(16));
    assert_eq!(spacing.menu_margin, egui::Margin::same(4));
    assert_eq!(spacing.indent, 12.0);
}

/// Noura's own background sits behind a note, so the editor's code colors
/// must read on that.
#[test]
fn every_code_color_reads_on_noura_s_background() {
    const CODE: &str = "```rust\n// a comment\nstruct Point;\nfn main() {\n    \
                        let total = 42;\n    let name = \"text\";\n}\n```\n";
    for (theme, palette) in [
        (Theme::Light, Palette::light()),
        (Theme::Dark, Palette::dark()),
    ] {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(800.0, 600.0))
            .with_theme(theme)
            .with_context_setup(|ctx| {
                app_ui::theme::install(ctx);
                editor_egui::register_font_system(ctx, editor_egui::fonts::default_font_database());
            })
            .build_ui_state(
                |ui, editor: &mut Option<Editor>| {
                    editor
                        .get_or_insert_with(|| Editor::new(ui.ctx(), CODE, MdConfig::default()))
                        .show(ui);
                },
                None,
            );
        harness.run_steps(3);
        assert_eq!(
            harness.ctx.global_style().visuals.panel_fill,
            palette.background
        );

        let spans = harness.state().as_ref().unwrap().painted_spans();
        for token in ["comment", "struct", "main", "Point", "42", "text"] {
            let color = spans
                .iter()
                .find(|span| span.glyphs && span.text.contains(token))
                .and_then(|span| span.color)
                .unwrap_or_else(|| panic!("{token} drew nothing"));
            assert!(
                contrast_ratio(color, palette.background).passes_normal_text(),
                "{token} in {color:?} on {:?}",
                palette.background
            );
        }
    }
}
