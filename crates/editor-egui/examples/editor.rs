//! Opens a Markdown file in an eframe window with the live-preview editor.
//!
//! ```sh
//! cargo run -p editor-egui --example editor -- path/to/note.md
//! ```
//!
//! Type to edit. Cmd+S (Ctrl+S elsewhere) writes the note back to the file.
//! Link lookups answer not found, and images show as placeholders.

use std::path::{Path, PathBuf};

use editor_egui::resolvers::OpenedLink;
use editor_egui::{Editor, MdConfig};

fn main() -> eframe::Result {
    let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: editor <file.md>");
        std::process::exit(2);
    };
    let text = match std::fs::read(&path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                eprintln!("{} is not UTF-8 text", path.display());
                std::process::exit(1);
            }
        },
        Err(err) => {
            eprintln!("can't read {}: {err}", path.display());
            std::process::exit(1);
        }
    };

    let title = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Editor".into());
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            let render_state = cc
                .wgpu_render_state
                .as_ref()
                .ok_or("the editor needs the wgpu renderer")?;
            let font_system = editor_egui::register_font_system(&cc.egui_ctx, font_database());
            editor_egui::register_render_callback_resources(
                &render_state.device,
                &render_state.queue,
                render_state.target_format,
                &mut render_state.renderer.write(),
                font_system,
                1,
            );
            let ext = path
                .extension()
                .map(|ext| ext.to_string_lossy().into_owned())
                .unwrap_or_default();
            let editor = Editor::new(
                &cc.egui_ctx,
                &text,
                MdConfig {
                    readonly: false,
                    ext,
                },
            );
            Ok(Box::new(ExampleApp {
                editor,
                path,
                status: String::new(),
            }))
        }),
    )
}

/// egui's bundled fonts first, then the system's fonts so text in other
/// scripts, such as Japanese, still finds glyphs.
fn font_database() -> glyphon::fontdb::Database {
    let mut db = editor_egui::fonts::default_font_database();
    db.load_system_fonts();
    db
}

struct ExampleApp {
    editor: Editor,
    path: PathBuf,
    status: String,
}

impl ExampleApp {
    fn save(&mut self) {
        let (seq, text) = self.editor.flush_for_save();
        match write_atomically(&self.path, text.as_bytes()) {
            Ok(()) => {
                self.editor.record_disk_save(seq, text);
                self.status = format!("Saved {}", self.path.display());
            }
            Err(err) => self.status = format!("Couldn't save: {err}"),
        }
    }
}

/// Writes `bytes` beside `path` and renames the copy over it, so a failed
/// write leaves the original file whole.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    // Keep the note's permissions rather than the temporary file's.
    if let Ok(metadata) = std::fs::metadata(path) {
        file.as_file().set_permissions(metadata.permissions())?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|err| err.error)?;
    Ok(())
}

impl eframe::App for ExampleApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let save = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S));
        if save {
            self.save();
        }

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(&self.status);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let response = self.editor.show(ui);
            if response.text_updated && self.status.starts_with("Saved") {
                self.status.clear();
            }
            if let Some(link) = response.opened_links.into_iter().last() {
                self.status = match link {
                    OpenedLink::File(file) => format!("Link to {file}"),
                    OpenedLink::External(url) => format!("Link to {url}"),
                    OpenedLink::NotFound { target } => format!("Nothing matches {target}"),
                };
            }
        });
    }
}
