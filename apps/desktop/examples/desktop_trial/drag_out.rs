//! Dragging a file out of the window into Finder or Explorer, through the
//! `drag` crate.

/// How a drag out of the window ended.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows")),
    allow(dead_code, reason = "Linux never drags out")
)]
pub enum DragOutcome {
    /// Another app accepted the file.
    Dropped,
    /// The drag ended without a drop, or the target refused it.
    Cancelled,
    /// The drag never started.
    Failed(String),
}

/// The picture under the pointer while a file is dragged out.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const DRAG_ICON: &[u8] = include_bytes!("drag-icon.png");

/// Starts a native drag of `path` (an absolute path) out of the main window.
/// `done` runs once the drag ends: later on macOS, before this returns on
/// Windows.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn start(
    frame: &eframe::Frame,
    path: &std::path::Path,
    done: impl Fn(DragOutcome) + Send + 'static,
) -> Result<(), String> {
    drag::start_drag(
        frame,
        drag::DragItem::Files(vec![path.to_owned()]),
        drag::Image::Raw(DRAG_ICON.to_vec()),
        move |result, _cursor| {
            done(match result {
                drag::DragResult::Dropped => DragOutcome::Dropped,
                drag::DragResult::Cancel => DragOutcome::Cancelled,
            });
        },
        drag::Options::default(),
    )
    .map_err(|error| error.to_string())
}

/// Linux: the `drag` crate drags out through a GTK window, and winit
/// doesn't make one.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn start(
    _frame: &eframe::Frame,
    _path: &std::path::Path,
    _done: impl Fn(DragOutcome) + Send + 'static,
) -> Result<(), String> {
    Err("dragging out isn't available on this platform".to_owned())
}
