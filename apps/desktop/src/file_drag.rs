//! Dragging files between the window and the file manager. Files dragged
//! in arrive through egui, and this module tells egui where the pointer is
//! while they hover, which winit 0.30 doesn't. A tree row dragged out of
//! the window becomes a native drag through the `drag` crate on macOS and
//! Windows.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use app_ui::NouraUi;
use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, RawInput, Rect};

/// Starts native drags, and adds to egui's input what the system keeps
/// from winit during a drag.
#[derive(Default)]
pub struct FileDrag {
    /// A native drag ended. The system kept its mouse-up, so egui still
    /// thinks the button is down.
    ended: Arc<AtomicBool>,
    /// Files hovered the window last frame, and egui got the pointer's
    /// place from the system rather than from winit.
    told: bool,
}

impl FileDrag {
    /// Add to this frame's input the pointer's place while the file manager
    /// drags files over the window, the pointer leaving with them, and the
    /// mouse-up a native drag kept.
    pub fn complete_input(&mut self, ctx: &egui::Context, raw_input: &mut RawInput) {
        let native = raw_input.viewport().native_pixels_per_point;
        let cursor = if raw_input.hovered_files.is_empty() && raw_input.dropped_files.is_empty() {
            None
        } else {
            native.and_then(cursor::on_screen)
        };
        let last = LastFrame {
            pointer: ctx.input(|input| input.pointer.latest_pos()),
            zoom: ctx.zoom_factor(),
            told: self.told,
        };
        self.told = complete(raw_input, &self.ended, &last, cursor);
    }

    /// Start the native drag of a tree row that left the window, if one is
    /// waiting.
    pub fn start(&self, ui: &mut NouraUi, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(request) = ui.take_drag_out() else {
            return;
        };
        if let Err(error) = native::start(frame, request.path, ctx, &self.ended) {
            tracing::warn!(%error, "could not start dragging a file out");
            ui.drag_out_failed();
        }
    }
}

/// What egui knew at the end of the last frame.
struct LastFrame {
    pointer: Option<Pos2>,
    zoom: f32,
    /// The pointer's place came from the system while files hovered.
    told: bool,
}

/// Add the release a native drag kept, the pointer leaving with the files
/// the file manager drags, and `cursor` as a pointer move when the system
/// reported one, to `raw_input`. Returns whether files hover the window and
/// egui now has the pointer's place from `cursor`.
fn complete(
    raw_input: &mut RawInput,
    ended: &AtomicBool,
    last: &LastFrame,
    cursor: Option<Pos2>,
) -> bool {
    let hovering = !raw_input.hovered_files.is_empty();
    let dropped = !raw_input.dropped_files.is_empty();
    // These happened before anything winit reports this frame, so a real
    // pointer move still wins.
    let mut before = Vec::new();
    let released = ended.swap(false, Ordering::AcqRel);
    if released {
        // egui needs a place for the release, and the last one it knew is
        // the best guess.
        before.push(Event::PointerButton {
            pos: last.pointer.unwrap_or_default(),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
    }
    // When the files leave the window, winit says only that. The pointer
    // egui has is the last place the system gave, near the edge they
    // crossed, and a row there would stay hovered until the mouse came
    // back. A release whose place is a guess mustn't leave the pointer
    // there either.
    let left = last.told && !hovering && !dropped;
    if left || (released && last.pointer.is_none()) {
        before.push(Event::PointerGone);
    }
    raw_input.events.splice(0..0, before);
    let Some(cursor) = cursor else {
        return false;
    };
    let viewport = raw_input.viewport();
    let (Some(inner), Some(native)) = (viewport.inner_rect, viewport.native_pixels_per_point)
    else {
        return false;
    };
    let pixels_per_point = native * last.zoom;
    raw_input.events.push(Event::PointerMoved(in_window(
        cursor,
        pixels_per_point,
        inner,
    )));
    hovering && !dropped
}

/// `cursor`, in physical pixels from the top-left corner of the main
/// display, in the points of the window whose content area is at `inner`.
fn in_window(cursor: Pos2, pixels_per_point: f32, inner: Rect) -> Pos2 {
    (cursor.to_vec2() / pixels_per_point).to_pos2() - inner.min.to_vec2()
}

/// Where the pointer is on screen, in physical pixels from the top-left
/// corner of the main display, as winit places windows.
mod cursor {
    use eframe::egui::Pos2;

    /// AppKit measures in points from the bottom-left corner of the main
    /// display, the one with the menu bar.
    #[cfg(target_os = "macos")]
    pub fn on_screen(native_pixels_per_point: f32) -> Option<Pos2> {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSEvent, NSScreen};
        let main_thread = MainThreadMarker::new()?;
        let main = NSScreen::screens(main_thread).firstObject()?;
        let height = main.frame().size.height;
        let location = NSEvent::mouseLocation();
        let points = Pos2::new(location.x as f32, (height - location.y) as f32);
        Some((points.to_vec2() * native_pixels_per_point).to_pos2())
    }

    #[cfg(target_os = "windows")]
    pub fn on_screen(_native_pixels_per_point: f32) -> Option<Pos2> {
        use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};
        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: GetCursorPos only writes the position into the POINT it
        // gets, which lives for the whole call.
        let found = unsafe { GetCursorPos(&mut point) } != 0;
        found.then(|| Pos2::new(point.x as f32, point.y as f32))
    }

    /// winit 0.30's X11 windows don't say where a file drag is, and Noura
    /// has no X11 connection of its own to ask, so files dropped there go
    /// to the top of the workspace. Its Wayland windows take no file drops
    /// at all.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub fn on_screen(_native_pixels_per_point: f32) -> Option<Pos2> {
        None
    }
}

/// The native drag out of the window.
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use eframe::egui;

    /// The picture under the pointer while a file is dragged out.
    const DRAG_ICON: &[u8] = include_bytes!("../assets/drag-icon.png");

    /// Start a native drag of `path` out of the main window, which hands the
    /// app it lands in a copy. When the drag ends, `ended` goes up and the
    /// window draws again: later on macOS, before this returns on Windows.
    pub fn start(
        frame: &eframe::Frame,
        path: PathBuf,
        ctx: &egui::Context,
        ended: &Arc<AtomicBool>,
    ) -> Result<(), drag::Error> {
        let ended = ended.clone();
        let ctx = ctx.clone();
        drag::start_drag(
            frame,
            drag::DragItem::Files(vec![path]),
            drag::Image::Raw(DRAG_ICON.to_vec()),
            move |_result, _cursor| {
                ended.store(true, Ordering::Release);
                ctx.request_repaint();
            },
            drag::Options::default(),
        )
    }
}

/// Linux has no native drag out of a winit window, and `app-core` never
/// asks for one there.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod native {
    use std::{
        path::PathBuf,
        sync::{Arc, atomic::AtomicBool},
    };

    use eframe::egui;

    pub fn start(
        _frame: &eframe::Frame,
        _path: PathBuf,
        _ctx: &egui::Context,
        _ended: &Arc<AtomicBool>,
    ) -> Result<(), &'static str> {
        Err("dragging out isn't available on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{ViewportId, pos2, vec2};

    /// A frame's input while the file manager drags a file over a window
    /// whose content area starts at (100, 50) on a display with two pixels
    /// per point.
    fn hovering() -> RawInput {
        let mut raw_input = RawInput::default();
        let viewport = raw_input.viewports.get_mut(&ViewportId::ROOT).unwrap();
        viewport.inner_rect = Some(Rect::from_min_size(pos2(100.0, 50.0), vec2(800.0, 600.0)));
        viewport.native_pixels_per_point = Some(2.0);
        raw_input.hovered_files.push(egui::HoveredFile {
            path: Some("/tmp/scan.pdf".into()),
            ..Default::default()
        });
        raw_input
    }

    fn unzoomed() -> LastFrame {
        LastFrame {
            pointer: None,
            zoom: 1.0,
            told: false,
        }
    }

    /// What egui knew after the file manager dragged a file over the window
    /// and the system said where the pointer was.
    fn after_hovering() -> LastFrame {
        LastFrame {
            pointer: Some(pos2(5.0, 300.0)),
            zoom: 1.0,
            told: true,
        }
    }

    #[test]
    fn the_pointer_follows_a_file_drag_in_window_points() {
        let mut raw_input = hovering();

        let told = complete(
            &mut raw_input,
            &AtomicBool::new(false),
            &unzoomed(),
            Some(pos2(300.0, 200.0)),
        );

        assert_eq!(raw_input.events, [Event::PointerMoved(pos2(50.0, 50.0))]);
        assert!(told);
    }

    #[test]
    fn files_that_leave_the_window_take_the_pointer_with_them() {
        let mut raw_input = hovering();
        raw_input.hovered_files.clear();

        let told = complete(
            &mut raw_input,
            &AtomicBool::new(false),
            &after_hovering(),
            None,
        );
        assert_eq!(raw_input.events, [Event::PointerGone]);
        assert!(!told);

        // Only the frame the files left in.
        let mut next = RawInput::default();
        complete(&mut next, &AtomicBool::new(false), &unzoomed(), None);
        assert!(next.events.is_empty());
    }

    #[test]
    fn the_mouse_coming_back_as_the_files_leave_puts_the_pointer_where_it_is() {
        let mut raw_input = RawInput::default();
        raw_input
            .events
            .push(Event::PointerMoved(pos2(40.0, 120.0)));

        complete(
            &mut raw_input,
            &AtomicBool::new(false),
            &after_hovering(),
            None,
        );

        assert_eq!(
            raw_input.events,
            [Event::PointerGone, Event::PointerMoved(pos2(40.0, 120.0))]
        );
    }

    #[test]
    fn dropped_files_leave_the_pointer_where_they_landed() {
        /// A file dropped from the file manager.
        #[derive(Debug)]
        struct Dropped;

        impl egui::DroppedFile for Dropped {
            fn path(&self) -> &std::path::Path {
                "/tmp/scan.pdf".as_ref()
            }

            fn bytes(&self) -> Result<Vec<u8>, String> {
                Ok(Vec::new())
            }
        }

        let mut raw_input = hovering();
        raw_input.hovered_files.clear();
        raw_input.dropped_files.push(std::sync::Arc::new(Dropped));

        let told = complete(
            &mut raw_input,
            &AtomicBool::new(false),
            &after_hovering(),
            Some(pos2(300.0, 200.0)),
        );
        assert_eq!(raw_input.events, [Event::PointerMoved(pos2(50.0, 50.0))]);
        assert!(!told);

        let mut next = RawInput::default();
        let last = LastFrame {
            told,
            ..after_hovering()
        };
        complete(&mut next, &AtomicBool::new(false), &last, None);
        assert!(next.events.is_empty());
    }

    #[test]
    fn where_the_system_cant_say_where_the_pointer_is_files_leaving_dont_move_it() {
        let mut raw_input = hovering();

        let told = complete(&mut raw_input, &AtomicBool::new(false), &unzoomed(), None);
        assert!(!told);

        let mut next = RawInput::default();
        complete(&mut next, &AtomicBool::new(false), &unzoomed(), None);
        assert!(next.events.is_empty());
    }

    #[test]
    fn the_pointer_follows_a_file_drag_at_the_windows_zoom() {
        let mut raw_input = hovering();
        let zoomed = LastFrame {
            pointer: None,
            zoom: 2.0,
            told: false,
        };

        complete(
            &mut raw_input,
            &AtomicBool::new(false),
            &zoomed,
            Some(pos2(800.0, 400.0)),
        );

        assert_eq!(raw_input.events, [Event::PointerMoved(pos2(100.0, 50.0))]);
    }

    #[test]
    fn the_end_of_a_native_drag_releases_the_button_egui_still_holds() {
        let ended = AtomicBool::new(true);
        let last = LastFrame {
            pointer: Some(pos2(5.0, 300.0)),
            zoom: 1.0,
            told: false,
        };
        let mut raw_input = RawInput::default();

        complete(&mut raw_input, &ended, &last, None);
        complete(&mut raw_input, &ended, &last, None);

        assert_eq!(
            raw_input.events,
            [Event::PointerButton {
                pos: pos2(5.0, 300.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }]
        );
    }

    #[test]
    fn a_native_drag_that_ends_where_egui_lost_the_pointer_releases_without_bringing_it_back() {
        let lost = LastFrame {
            pointer: None,
            zoom: 1.0,
            told: false,
        };
        let mut raw_input = RawInput::default();

        complete(&mut raw_input, &AtomicBool::new(true), &lost, None);

        let release = |pos| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        };
        assert_eq!(raw_input.events, [release(Pos2::ZERO), Event::PointerGone]);

        // The row came back over the window as files and left again in the
        // frame the drag ended.
        let mut raw_input = RawInput::default();
        complete(
            &mut raw_input,
            &AtomicBool::new(true),
            &after_hovering(),
            None,
        );
        assert_eq!(
            raw_input.events,
            [release(pos2(5.0, 300.0)), Event::PointerGone]
        );
    }

    #[test]
    fn without_a_file_drag_the_pointer_stays_where_winit_put_it() {
        let mut raw_input = RawInput::default();

        complete(&mut raw_input, &AtomicBool::new(false), &unzoomed(), None);

        assert!(raw_input.events.is_empty());
    }
}
