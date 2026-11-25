//! On macOS the window's close, minimize and zoom buttons sit in the
//! rail's top row, closer together than AppKit puts them, where `app-ui`
//! says: see [`app_ui::window_buttons`]. Other platforms draw their own
//! title bar, and nothing changes there.
//!
//! AppKit puts the buttons back whenever it lays out the title bar: as
//! the window resizes, gains or loses focus, changes its title or leaves
//! full screen. Each of those moves changes a button's frame, which
//! AppKit announces, and Noura moves the button back then, before the
//! window draws. Each frame places them too, for a new window and for a
//! new zoom level. In full screen the buttons belong to the menu bar, and
//! stay where AppKit puts them.

use eframe::egui::Context;

/// Put every window's buttons in its rail.
#[cfg(target_os = "macos")]
pub fn place(ctx: &Context) {
    mac::place(ctx.zoom_factor());
}

#[cfg(not(target_os = "macos"))]
pub fn place(_ctx: &Context) {}

#[cfg(target_os = "macos")]
pub(crate) use mac::is_noura_window;

#[cfg(target_os = "macos")]
mod mac {
    use std::{
        cell::Cell,
        ptr::NonNull,
        sync::{
            Once,
            atomic::{AtomicU32, Ordering},
        },
    };

    use block2::RcBlock;
    use objc2::{ClassType, MainThreadMarker, rc::Retained, runtime::NSObjectProtocol};
    use objc2_app_kit::{
        NSApplication, NSButton, NSPanel, NSView, NSViewFrameDidChangeNotification, NSWindow,
        NSWindowButton, NSWindowStyleMask,
    };
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize};

    /// The windows' zoom factor, as `f32` bits, for the frame changes
    /// AppKit announces outside a frame.
    static ZOOM: AtomicU32 = AtomicU32::new(0x3f80_0000);

    thread_local! {
        /// Moving a button announces its own frame change: the move in
        /// progress handles it.
        static PLACING: Cell<bool> = const { Cell::new(false) };
    }

    pub fn place(zoom: f32) {
        let Some(main_thread) = MainThreadMarker::new() else {
            return;
        };
        ZOOM.store(zoom.to_bits(), Ordering::Relaxed);
        static OBSERVE: Once = Once::new();
        OBSERVE.call_once(observe_frames);
        for window in NSApplication::sharedApplication(main_thread).windows() {
            place_in(&window);
        }
    }

    /// Move the buttons back whenever a view in one of Noura's windows,
    /// such as a button or the title bar, changes its frame. This
    /// observes for the app's whole life.
    fn observe_frames() {
        let block = RcBlock::new(|notification: NonNull<NSNotification>| {
            // SAFETY: AppKit hands the block a live notification.
            let notification = unsafe { notification.as_ref() };
            let view = notification
                .object()
                .and_then(|object| object.downcast::<NSView>().ok());
            if let Some(window) = view.and_then(|view| view.window()) {
                place_in(&window);
            }
        });
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: the name is AppKit's, no object or queue narrows it,
        // and AppKit posts frame changes on the main thread, where the
        // block runs.
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSViewFrameDidChangeNotification),
                None,
                None,
                &block,
            )
        };
        std::mem::forget(observer);
    }

    /// Noura's windows run their content under a transparent title bar.
    /// Panels, such as the folder picker, don't count.
    pub(crate) fn is_noura_window(window: &NSWindow) -> bool {
        !window.isKindOfClass(NSPanel::class())
            && window
                .styleMask()
                .contains(NSWindowStyleMask::FullSizeContentView)
            && window.titlebarAppearsTransparent()
    }

    fn place_in(window: &NSWindow) {
        if PLACING.get()
            || !is_noura_window(window)
            || window.styleMask().contains(NSWindowStyleMask::FullScreen)
        {
            return;
        }
        let buttons = [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ]
        .map(|kind| window.standardWindowButton(kind));
        let [Some(close), Some(minimize), Some(zoom)] = buttons else {
            return;
        };
        PLACING.set(true);
        place_buttons(window, [close, minimize, zoom]);
        PLACING.set(false);
    }

    fn place_buttons(window: &NSWindow, buttons: [Retained<NSButton>; 3]) {
        // SAFETY: the buttons and their title bar belong to the window,
        // which lives through this call.
        let Some(title_bar) = (unsafe { buttons[0].superview() }) else {
            return;
        };
        let zoom = f32::from_bits(ZOOM.load(Ordering::Relaxed));
        let spot = app_ui::window_buttons(zoom);
        let size = buttons[0].frame().size;
        let (width, height) = (size.width, size.height);
        let gap = f64::from(spot.gap);
        let row_width = 3.0 * width + 2.0 * gap;
        // Where the first button's top-left corner goes, from the
        // window's top-left corner.
        let left = (f64::from(spot.center.x) - row_width / 2.0).round();
        let top = (f64::from(spot.center.y) - height / 2.0).round();
        grow_title_bar(window, &title_bar, top + height);
        // The window's own coordinates start at its bottom-left corner,
        // and a frame's origin is the corner nearest its superview's.
        let window_height = window.frame().size.height;
        let y = if title_bar.isFlipped() {
            window_height - top
        } else {
            window_height - top - height
        };
        for (index, button) in buttons.iter().enumerate() {
            let x = left + index as f64 * (width + gap);
            let origin = title_bar.convertPoint_fromView(NSPoint::new(x, y), None);
            if button.frame().origin != origin {
                button.setFrameOrigin(origin);
            }
        }
    }

    /// AppKit's title bar is 32 points tall. At a large zoom the rail's
    /// top row is taller, and the title bar's container, at the top of
    /// the window, grows down to `bottom` to hold the buttons.
    fn grow_title_bar(window: &NSWindow, title_bar: &NSView, bottom: f64) {
        // SAFETY: the title bar's container belongs to the window, which
        // lives through this call.
        let Some(container) = (unsafe { title_bar.superview() }) else {
            return;
        };
        let frame = container.frame();
        if frame.size.height >= bottom {
            return;
        }
        let window_height = window.frame().size.height;
        let grown = NSRect::new(
            NSPoint::new(frame.origin.x, window_height - bottom),
            NSSize::new(frame.size.width, bottom),
        );
        container.setFrame(grown);
    }
}
