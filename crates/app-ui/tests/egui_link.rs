//! Link check for trial S0: egui 0.37 lays out text with its bundled fonts.

#[test]
fn egui_lays_out_a_label_with_its_default_fonts() {
    let ctx = egui::Context::default();
    let mut label_width = 0.0;
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        label_width = ui.label("Noura").rect.width();
    });

    assert!(label_width > 0.0);
    // The first frame uploads the font atlas built from egui's bundled fonts.
    assert!(!output.textures_delta.set.is_empty());
    let shapes = std::mem::take(&mut output.shapes);
    assert!(!ctx.tessellate(shapes, output.pixels_per_point).is_empty());
    output.drop_without_applying_deltas();
}
