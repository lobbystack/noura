//! The Noura desktop app.

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_ui_native("Noura", options, |_ui, _frame| {})
}
