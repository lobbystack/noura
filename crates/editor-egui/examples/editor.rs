//! Opens a Markdown file in an eframe window with the live-preview editor.
//!
//! ```sh
//! cargo run -p editor-egui --example editor -- path/to/note.md
//! ```
//!
//! Type to edit. Cmd+S (Ctrl+S elsewhere) writes the note back to the file.
//! If another app changed the file since it was opened or last saved, the
//! save merges those changes in first instead of overwriting them. Link
//! lookups answer not found, and images draw as not found.

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
                file: NoteFile::new(path, text.into_bytes()),
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
    file: NoteFile,
    status: String,
}

impl ExampleApp {
    fn save(&mut self) {
        self.status = match self.file.save(&mut self.editor) {
            Ok(Saved { merged: false }) => format!("Saved {}", self.file.path.display()),
            Ok(Saved { merged: true }) => format!(
                "Saved {}, with the changes made outside the editor merged in",
                self.file.path.display()
            ),
            Err(err) => format!("Couldn't save: {err}"),
        };
    }
}

/// The file a note was opened from, and the bytes last read from or written
/// to it.
struct NoteFile {
    path: PathBuf,
    on_disk: Vec<u8>,
}

/// What a save did.
#[derive(Debug, PartialEq)]
struct Saved {
    /// The file had changed since the last load or save, and the save merged
    /// those changes into the note before writing it.
    merged: bool,
}

impl NoteFile {
    fn new(path: PathBuf, on_disk: Vec<u8>) -> Self {
        Self { path, on_disk }
    }

    /// Writes the editor's text to the file. Changes another app made to the
    /// file since the last load or save merge into the note first, so the
    /// save doesn't overwrite them. A symlink stays a link: the save writes
    /// the file it points to. Returns once the bytes and the directory entry
    /// are on disk.
    fn save(&mut self, editor: &mut Editor) -> Result<Saved, String> {
        let target = match std::fs::canonicalize(&self.path) {
            Ok(target) => target,
            // Deleted since it was opened: write it again where it was.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => self.path.clone(),
            Err(err) => return Err(err.to_string()),
        };

        let mut merged = false;
        match std::fs::read(&target) {
            Ok(bytes) if bytes != self.on_disk => {
                let Ok(text) = String::from_utf8(bytes) else {
                    return Err(format!(
                        "another app changed {} and it isn't UTF-8 text any more",
                        target.display()
                    ));
                };
                editor.apply_disk_version(text.clone());
                self.on_disk = text.into_bytes();
                merged = true;
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.to_string()),
        }

        let (seq, text) = editor.flush_for_save();
        write_atomically(&target, text.as_bytes()).map_err(|err| err.to_string())?;
        editor.record_disk_save(seq, text.clone());
        self.on_disk = text.into_bytes();
        Ok(Saved { merged })
    }
}

/// Writes `bytes` to a temporary file beside `path`, syncs it, renames it
/// over `path` and syncs the directory, so a failed write leaves the
/// original whole. On Unix the file keeps its permissions.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut file = atomic_write_file::AtomicWriteFile::open(path)?;
    file.write_all(bytes)?;
    file.commit()
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

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::offset_types::Grapheme;
    use editor_egui::input::{Event, Location, Region};

    /// An editor on the note at `path`, with the context its frames run in.
    fn open(path: &Path) -> (egui::Context, Editor, NoteFile) {
        let bytes = std::fs::read(path).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        let ctx = egui::Context::default();
        editor_egui::register_font_system(&ctx, editor_egui::fonts::default_font_database());
        let editor = Editor::new(&ctx, &text, MdConfig::default());
        (ctx, editor, NoteFile::new(path.to_path_buf(), bytes))
    }

    /// Types `text` at grapheme `at` and runs a frame so it applies.
    fn type_at(ctx: &egui::Context, editor: &mut Editor, at: usize, text: &str) {
        editor.push_event(Event::Replace {
            region: Region::Location(Location::Grapheme(Grapheme(at))),
            text: text.into(),
            advance_cursor: true,
        });
        ctx.run_ui(egui::RawInput::default(), |ui| {
            editor.show(ui);
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn saving_writes_the_edited_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "# Title\n").unwrap();
        let (ctx, mut editor, mut file) = open(&path);

        type_at(&ctx, &mut editor, 7, " here");
        assert_eq!(file.save(&mut editor), Ok(Saved { merged: false }));

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Title here\n");
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1, "no temporary file stays behind");
    }

    #[test]
    fn saving_merges_an_outside_edit_instead_of_overwriting_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "first\n\nlast\n").unwrap();
        let (ctx, mut editor, mut file) = open(&path);

        type_at(&ctx, &mut editor, 5, " line");
        std::fs::write(&path, "first\n\nlast\n\nadded elsewhere\n").unwrap();
        assert_eq!(file.save(&mut editor), Ok(Saved { merged: true }));

        let expected = "first line\n\nlast\n\nadded elsewhere\n";
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        assert_eq!(editor.text(), expected);

        // The merged text is now the base: saving again merges nothing.
        assert_eq!(file.save(&mut editor), Ok(Saved { merged: false }));
    }

    #[cfg(unix)]
    #[test]
    fn saving_keeps_the_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        std::fs::write(&path, "text\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let (ctx, mut editor, mut file) = open(&path);

        type_at(&ctx, &mut editor, 4, "!");
        file.save(&mut editor).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_symlink_writes_its_target_and_keeps_the_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.md");
        let link = dir.path().join("link.md");
        std::fs::write(&target, "linked\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let (ctx, mut editor, mut file) = open(&link);

        type_at(&ctx, &mut editor, 6, " note");
        file.save(&mut editor).unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "linked note\n");
    }
}
