//! On macOS a Noura window's content runs under a transparent title bar,
//! so the title bar row Noura draws, tabs included, sits in the band at
//! the window's top where AppKit moves the window. AppKit moves it on a
//! drag that starts there unless the view under the pointer says it
//! handles the mouse down itself, and winit's view doesn't say so. A tab
//! you dragged to reorder moved the window instead, and the tab bar never
//! saw the drag.
//!
//! So winit's view says it handles every mouse down, and `app-ui` decides
//! what moves the window: an empty part of the title bar row asks for a
//! move with `ViewportCommand::StartDrag`, as on Windows and Linux. Other
//! platforms need nothing here.

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

#[cfg(target_os = "macos")]
mod mac {
    use std::{
        ffi::CString,
        sync::atomic::{AtomicBool, Ordering},
    };

    use objc2::{
        ClassType,
        encode::Encode,
        ffi,
        runtime::{AnyClass, AnyObject, Bool, Imp, Sel},
        sel,
    };
    use objc2_app_kit::NSView;
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
}
