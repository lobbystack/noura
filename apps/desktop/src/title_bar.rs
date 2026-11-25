//! What the title bar row Noura draws does to its window, beyond what
//! `app-ui` asks of egui: a double-click does what the system's setting
//! says, and on macOS only Noura decides what a drag there moves.
//!
//! On macOS a Noura window's content runs under a transparent title bar,
//! so the row, tabs included, sits in the band at the window's top where
//! AppKit moves the window. AppKit moves it on a drag that starts there
//! unless the view under the pointer says it handles the mouse down
//! itself, and winit's view doesn't say so. A tab you dragged to reorder
//! moved the window instead, and the tab bar never saw the drag. So
//! winit's view says it handles every mouse down, and an empty part of
//! the row asks for a move with `ViewportCommand::StartDrag`, as on
//! Windows and Linux.

use eframe::egui::{Context, ViewportId};
use winit::raw_window_handle::HasWindowHandle;

/// Keep AppKit from moving Noura's windows on its own. `window` is one of
/// them: every window's view shares the class this changes, so one call
/// does it, and later calls return at once.
#[cfg(target_os = "macos")]
pub fn claim_mouse_downs(window: &impl HasWindowHandle) {
    mac::claim_mouse_downs(window);
}

#[cfg(not(target_os = "macos"))]
pub fn claim_mouse_downs(_window: &impl HasWindowHandle) {}

/// Do what the system's setting says a double-click on a title bar does,
/// when one on the title bar row of the window `viewport` asked for it.
/// Call it in that window's frame, after its UI.
pub fn double_click(ctx: &Context, viewport: ViewportId) {
    if !app_ui::take_title_bar_double_click(ctx, viewport) {
        return;
    }
    #[cfg(target_os = "macos")]
    mac::double_click();
    // Windows and Linux maximize the window, or restore it.
    #[cfg(not(target_os = "macos"))]
    {
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(eframe::egui::ViewportCommand::Maximized(!maximized));
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::{
        ffi::CString,
        sync::atomic::{AtomicBool, Ordering},
    };

    use objc2::{
        ClassType, MainThreadMarker,
        encode::Encode,
        ffi, msg_send,
        runtime::{AnyClass, AnyObject, Bool, Imp, NSObjectProtocol, Sel},
        sel,
    };
    use objc2_app_kit::{NSApplication, NSView, NSWindow};
    use objc2_foundation::{NSUserDefaults, ns_string};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// Whether winit's view class has the answer, or refused it.
    static DONE: AtomicBool = AtomicBool::new(false);

    /// `-[NSView mouseDownCanMoveWindow]` for winit's view: no, Noura
    /// handles every mouse down.
    extern "C-unwind" fn mouse_down_cannot_move_window(_view: &AnyObject, _cmd: Sel) -> Bool {
        Bool::NO
    }

    pub fn claim_mouse_downs(window: &impl HasWindowHandle) {
        if DONE.load(Ordering::Relaxed) {
            return;
        }
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: the handle borrows the window, so its view, winit's,
        // lives through this call.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        DONE.store(true, Ordering::Relaxed);
        let added = winit_view_class(view).is_some_and(add_answer);
        if !added {
            tracing::warn!(
                class = %AnyObject::class(view).name().to_string_lossy(),
                "a drag on a tab may move the window: the window's view already \
                 answers whether a mouse down moves it"
            );
        }
    }

    /// winit's view class: the class of `view`, or the class it descends
    /// from, that subclasses `NSView` directly. AccessKit gives each view
    /// a subclass of its own, which inherits what winit's class answers.
    fn winit_view_class(view: &NSView) -> Option<&'static AnyClass> {
        let ns_view = NSView::class();
        let mut class = AnyObject::class(view);
        loop {
            let superclass = class.superclass()?;
            if superclass == ns_view {
                return Some(class);
            }
            class = superclass;
        }
    }

    /// Add the answer to `class`. Fails when the class has its own answer,
    /// which this leaves alone.
    fn add_answer(class: &AnyClass) -> bool {
        let types = format!(
            "{}{}{}",
            Bool::ENCODING,
            <&AnyObject>::ENCODING,
            Sel::ENCODING
        );
        let Ok(types) = CString::new(types) else {
            return false;
        };
        let answer: extern "C-unwind" fn(&AnyObject, Sel) -> Bool = mouse_down_cannot_move_window;
        // SAFETY: an Objective-C method's implementation takes the receiver
        // and the selector, then the arguments, here none, and the type
        // string says it returns a BOOL, as `-mouseDownCanMoveWindow`
        // does. `Imp` is the runtime's untyped function pointer, which the
        // runtime calls with that signature.
        let imp = unsafe {
            std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel) -> Bool, Imp>(answer)
        };
        // SAFETY: `class` is a registered class, which may gain methods.
        // `class_addMethod` adds to the class only, never replacing a
        // method the class defines, and copies the type string.
        let added = unsafe {
            ffi::class_addMethod(
                std::ptr::from_ref(class).cast_mut(),
                sel!(mouseDownCanMoveWindow),
                imp,
                types.as_ptr(),
            )
        };
        added.as_bool()
    }

    /// What a double-click on a title bar does, as System Settings, Desktop
    /// & Dock, sets it.
    #[derive(Debug, PartialEq, Eq)]
    enum Action {
        Zoom,
        Fill,
        Minimize,
        Nothing,
    }

    /// The action `AppleActionOnDoubleClick` names: System Settings writes
    /// Zoom as `Maximize`. Older systems set only the flag
    /// `AppleMiniaturizeOnDoubleClick`. Neither set means Zoom, and a value
    /// this doesn't know does nothing.
    fn action(setting: Option<&str>, minimize: bool) -> Action {
        match setting {
            Some("Maximize") => Action::Zoom,
            Some("Fill") => Action::Fill,
            Some("Minimize") => Action::Minimize,
            Some(_) => Action::Nothing,
            None if minimize => Action::Minimize,
            None => Action::Zoom,
        }
    }

    pub fn double_click() {
        let Some(main_thread) = MainThreadMarker::new() else {
            return;
        };
        // The double-click's first click made its window the key window.
        let Some(window) = NSApplication::sharedApplication(main_thread).keyWindow() else {
            return;
        };
        if !crate::window_buttons::is_noura_window(&window) {
            return;
        }
        let defaults = NSUserDefaults::standardUserDefaults();
        let setting = defaults
            .stringForKey(ns_string!("AppleActionOnDoubleClick"))
            .map(|setting| setting.to_string());
        let minimize = defaults.boolForKey(ns_string!("AppleMiniaturizeOnDoubleClick"));
        match action(setting.as_deref(), minimize) {
            Action::Zoom => window.performZoom(None),
            Action::Fill => fill(&window),
            Action::Minimize => window.performMiniaturize(None),
            Action::Nothing => {}
        }
    }

    /// Fill the screen between the menu bar and the Dock, as AppKit's own
    /// title bar does on macOS 15 and later. AppKit has no public call for
    /// it: `_zoomFill:` is the one its title bar and Chromium make. Without
    /// it, zoom.
    fn fill(window: &NSWindow) {
        let fill = sel!(_zoomFill:);
        if window.respondsToSelector(fill) {
            // SAFETY: the window answers `_zoomFill:`, which takes a
            // sender, here none, and returns nothing.
            unsafe {
                let _: () = msg_send![window, _zoomFill: std::ptr::null::<AnyObject>()];
            }
        } else {
            window.performZoom(None);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Action, action};

        #[test]
        fn a_double_click_does_what_system_settings_says() {
            assert_eq!(action(Some("Maximize"), false), Action::Zoom);
            assert_eq!(action(Some("Fill"), false), Action::Fill);
            assert_eq!(action(Some("Minimize"), false), Action::Minimize);
            assert_eq!(action(Some("None"), false), Action::Nothing);
            // The newer setting wins over the older flag.
            assert_eq!(action(Some("Maximize"), true), Action::Zoom);
            // Only the older flag set: minimize; neither: zoom.
            assert_eq!(action(None, true), Action::Minimize);
            assert_eq!(action(None, false), Action::Zoom);
            // A value macOS adds later does nothing.
            assert_eq!(action(Some("Tile"), false), Action::Nothing);
        }
    }
}
