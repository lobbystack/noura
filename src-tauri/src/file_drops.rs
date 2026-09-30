//! Dragging files between the file tree and the system file manager.
//!
//! Drops from outside arrive as window events. Their absolute paths stay in
//! this process: the page gets the pointer position and a drop ID, picks the
//! folder under the pointer, and asks for that drop to be copied there. A
//! drag out of the tree becomes a native drag of the file on disk, so the
//! file manager copies it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use local_core::{CoreError, ImportResult};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, State, WebviewWindow, Window};

use super::{AppState, current_engine};

const DROP_EVENT: &str = "noura://file-drop";

/// Files dragged in from outside, waiting for the page to pick a folder.
#[derive(Default)]
pub struct FileDrops {
    next_id: AtomicU64,
    /// Whether the drag over the window carries files. Internal drags of
    /// page elements also raise drag events, with no paths.
    carrying_files: Mutex<bool>,
    pending: Mutex<Option<(u64, Vec<PathBuf>)>>,
}

/// What the page hears about a drag from outside. Positions are in CSS
/// pixels from the top-left of the page.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum FileDropEvent {
    Over {
        x: f64,
        y: f64,
    },
    Leave,
    #[serde(rename_all = "camelCase")]
    Drop {
        drop_id: u64,
        x: f64,
        y: f64,
        /// Set when every dropped item is already in the open workspace,
        /// as when a row dragged out of the tree comes back: the page moves
        /// these paths instead of copying them.
        workspace_paths: Option<Vec<String>>,
    },
}

pub fn window_event(window: &Window, event: &DragDropEvent) {
    let state = window.state::<AppState>();
    let drops = &state.file_drops;
    let at = |position: &tauri::PhysicalPosition<f64>| page_position(window, position);
    let set_carrying = |value: bool| {
        if let Ok(mut carrying) = drops.carrying_files.lock() {
            *carrying = value;
        }
    };
    let message = match event {
        DragDropEvent::Enter { paths, position } => {
            set_carrying(!paths.is_empty());
            if paths.is_empty() {
                return;
            }
            let (x, y) = at(position);
            FileDropEvent::Over { x, y }
        }
        DragDropEvent::Over { position } => {
            if !drops.carrying_files.lock().is_ok_and(|value| *value) {
                return;
            }
            let (x, y) = at(position);
            FileDropEvent::Over { x, y }
        }
        DragDropEvent::Drop { paths, position } => {
            set_carrying(false);
            if paths.is_empty() {
                return;
            }
            let drop_id = drops.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            let workspace_paths =
                current_root(&state).and_then(|root| workspace_relative_paths(&root, paths));
            if let Ok(mut pending) = drops.pending.lock() {
                *pending = Some((drop_id, paths.clone()));
            }
            let (x, y) = at(position);
            FileDropEvent::Drop {
                drop_id,
                x,
                y,
                workspace_paths,
            }
        }
        DragDropEvent::Leave => {
            set_carrying(false);
            FileDropEvent::Leave
        }
        _ => return,
    };
    if let Err(error) = window.emit(DROP_EVENT, message) {
        log::warn!("could not deliver a file drop: {error}");
    }
}

/// The drop position in CSS pixels. Tauri types it as physical pixels, but
/// only Windows reports physical pixels; macOS and GTK already report
/// logical points.
fn page_position(window: &Window, position: &tauri::PhysicalPosition<f64>) -> (f64, f64) {
    if cfg!(target_os = "windows") {
        let scale = window.scale_factor().unwrap_or(1.0);
        let position = position.to_logical::<f64>(scale);
        (position.x, position.y)
    } else {
        (position.x, position.y)
    }
}

fn current_root(state: &AppState) -> Option<PathBuf> {
    let engine = state.engine.lock().ok()?.clone()?;
    std::fs::canonicalize(engine.root()).ok()
}

/// The workspace paths of `paths` when every one is inside `root`.
fn workspace_relative_paths(root: &Path, paths: &[PathBuf]) -> Option<Vec<String>> {
    paths
        .iter()
        .map(|path| {
            let path = std::fs::canonicalize(path).ok()?;
            let relative = path.strip_prefix(root).ok()?.to_str()?;
            (!relative.is_empty()).then(|| relative.replace('\\', "/"))
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDropInput {
    drop_id: u64,
    folder: String,
}

/// Copy the files of drop `drop_id` into `folder`. Each drop can be used
/// once; a newer drop replaces an unused one.
#[tauri::command(async)]
pub fn files_import_drop(
    state: State<AppState>,
    input: ImportDropInput,
) -> Result<ImportResult, CoreError> {
    let operation = "files_import";
    let paths = state
        .file_drops
        .pending
        .lock()
        .ok()
        .and_then(|mut pending| {
            pending
                .take_if(|(id, _)| *id == input.drop_id)
                .map(|(_, paths)| paths)
        })
        .ok_or_else(|| {
            CoreError::validation(
                "drop_expired",
                "Those files are no longer available. Drag them in again.",
                operation,
            )
        })?;
    let engine = current_engine(&state, operation)?;
    engine.import_external(&paths, &input.folder)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DragOutInput {
    relative_paths: Vec<String>,
    /// A PNG the page drew to show under the pointer, such as the file's
    /// name, at `image_scale` pixels per point. The app icon when absent.
    #[serde(default)]
    image: Option<Vec<u8>>,
    #[serde(default)]
    image_scale: Option<f64>,
}

/// Largest drag image accepted from the page.
const MAX_DRAG_IMAGE_BYTES: usize = 1024 * 1024;

#[cfg(desktop)]
const DRAG_IMAGE: &[u8] = include_bytes!("../icons/64x64.png");

/// Start a native drag of workspace files, so they can be dropped in the
/// file manager or another app, which receive a copy. Call it while the
/// mouse button is still down.
#[tauri::command(async)]
pub fn files_drag_out(
    app: AppHandle,
    window: WebviewWindow,
    state: State<AppState>,
    input: DragOutInput,
) -> Result<(), CoreError> {
    let operation = "files_drag_out";
    let engine = current_engine(&state, operation)?;
    let paths = input
        .relative_paths
        .iter()
        .map(|relative| {
            let path = engine.resolve_managed_path(relative, operation)?;
            std::fs::symlink_metadata(&path)
                .map_err(|error| CoreError::io(error, operation, Some(relative)))?;
            Ok(path)
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    if paths.is_empty() {
        return Ok(());
    }
    let image = input
        .image
        .and_then(|png| png_with_scale(png, input.image_scale.unwrap_or(1.0)));
    app.run_on_main_thread(move || start_native_drag(&window, paths, image))
        .map_err(|_| drag_failed(operation))
}

#[cfg(mobile)]
fn start_native_drag(_window: &WebviewWindow, _paths: Vec<PathBuf>, _image: Option<Vec<u8>>) {}

#[cfg(desktop)]
fn start_native_drag(window: &WebviewWindow, paths: Vec<PathBuf>, image: Option<Vec<u8>>) {
    let item = drag::DragItem::Files(paths);
    let image = drag::Image::Raw(image.unwrap_or_else(|| DRAG_IMAGE.to_vec()));
    let options = drag::Options::default();
    #[cfg(target_os = "linux")]
    let result = match window.gtk_window() {
        Ok(gtk_window) => drag::start_drag(&gtk_window, item, image, |_, _| {}, options),
        Err(_) => Err(drag::Error::UnsupportedWindowHandle),
    };
    #[cfg(not(target_os = "linux"))]
    let result = drag::start_drag(window, item, image, |_, _| {}, options);
    if let Err(error) = result {
        log::warn!("could not start dragging files out: {error}");
    }
}

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// `png` with a `pHYs` chunk saying it holds `scale` pixels per point, so
/// macOS shows a Retina image at its intended size instead of twice as
/// large. None when `png` isn't a PNG this can safely tag.
fn png_with_scale(png: Vec<u8>, scale: f64) -> Option<Vec<u8>> {
    // Signature, then the IHDR chunk: length, type, 13 bytes, checksum.
    const IHDR_END: usize = 8 + 4 + 4 + 13 + 4;
    if png.len() > MAX_DRAG_IMAGE_BYTES
        || png.len() < IHDR_END
        || !png.starts_with(PNG_SIGNATURE)
        || &png[12..16] != b"IHDR"
        || !(1.0..=4.0).contains(&scale)
    {
        return None;
    }
    // Points are 1/72 inch; pHYs counts pixels per meter.
    let per_meter = (scale * 72.0 / 0.0254).round() as u32;
    let mut chunk = Vec::with_capacity(21);
    chunk.extend_from_slice(&9u32.to_be_bytes());
    chunk.extend_from_slice(b"pHYs");
    chunk.extend_from_slice(&per_meter.to_be_bytes());
    chunk.extend_from_slice(&per_meter.to_be_bytes());
    chunk.push(1);
    let checksum = crc32(&chunk[4..]);
    chunk.extend_from_slice(&checksum.to_be_bytes());
    let mut tagged = Vec::with_capacity(png.len() + chunk.len());
    tagged.extend_from_slice(&png[..IHDR_END]);
    tagged.extend_from_slice(&chunk);
    tagged.extend_from_slice(&png[IHDR_END..]);
    Some(tagged)
}

/// The CRC-32 that PNG chunks carry.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn drag_failed(operation: &str) -> CoreError {
    CoreError::new(
        "drag_failed",
        local_core::ErrorCategory::Transient,
        "The files couldn't be dragged out. Try again.",
        operation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_images_get_a_valid_scale_chunk() {
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        let png = include_bytes!("../icons/32x32.png").to_vec();
        let tagged = png_with_scale(png.clone(), 2.0).unwrap();
        assert_eq!(tagged.len(), png.len() + 21);
        let chunk = &tagged[33..54];
        assert_eq!(&chunk[4..8], b"pHYs");
        assert_eq!(u32::from_be_bytes(chunk[8..12].try_into().unwrap()), 5669);
        assert_eq!(
            u32::from_be_bytes(chunk[17..21].try_into().unwrap()),
            crc32(&chunk[4..17])
        );
        assert!(png_with_scale(b"not a png".to_vec(), 2.0).is_none());
        assert!(png_with_scale(png, 10.0).is_none());
    }

    #[test]
    fn only_drops_entirely_from_the_workspace_become_moves() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(workspace.path()).unwrap();
        std::fs::create_dir(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/a.md"), "a").unwrap();
        std::fs::write(outside.path().join("b.md"), "b").unwrap();

        assert_eq!(
            workspace_relative_paths(&root, &[root.join("notes/a.md"), root.join("notes")]),
            Some(vec!["notes/a.md".to_owned(), "notes".to_owned()])
        );
        assert_eq!(
            workspace_relative_paths(
                &root,
                &[root.join("notes/a.md"), outside.path().join("b.md")]
            ),
            None
        );
        assert_eq!(
            workspace_relative_paths(&root, std::slice::from_ref(&root)),
            None
        );
    }
}
