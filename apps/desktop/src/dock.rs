//! A click on Noura in the Dock, on macOS. Noura stays open once its last
//! window closes, as Safari does, and a click on its Dock icon shows the
//! window again. winit doesn't pass that click on, so Noura answers the
//! reopen Apple event itself. A click while windows show brings the one
//! that had focus last to the front.

use std::sync::atomic::{AtomicBool, Ordering};

static REOPEN: AtomicBool = AtomicBool::new(false);

/// Whether the Dock asked for a window since the last call.
pub fn take_reopen() -> bool {
    REOPEN.swap(false, Ordering::SeqCst)
}

/// Ask for a window as a click on the Dock icon does.
#[cfg(test)]
pub fn ask_reopen() {
    REOPEN.store(true, Ordering::SeqCst);
}

/// Listen for clicks on the Dock icon, and wake the windows with `ctx`
/// for each. Elsewhere than macOS nothing calls.
pub fn listen(ctx: &egui::Context) {
    #[cfg(target_os = "macos")]
    mac::listen(ctx.clone());
    #[cfg(not(target_os = "macos"))]
    let _ = ctx;
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::{Once, OnceLock, atomic::Ordering};

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
    use objc2::{AllocAnyThread, class, define_class, msg_send, sel};

    /// The Apple event class and ID of a reopen: `aevt` and `rapp`.
    const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
    const REOPEN_APPLICATION: u32 = u32::from_be_bytes(*b"rapp");

    static WAKE: OnceLock<egui::Context> = OnceLock::new();
    static SETUP: Once = Once::new();

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and the class
        // keeps no state of its own.
        #[unsafe(super(NSObject))]
        #[name = "NouraReopenHandler"]
        struct Handler;

        unsafe impl NSObjectProtocol for Handler {}

        impl Handler {
            /// The Dock icon was clicked.
            #[unsafe(method(handleReopen:withReplyEvent:))]
            fn handle_reopen(&self, _event: &AnyObject, _reply: &AnyObject) {
                super::REOPEN.store(true, Ordering::SeqCst);
                if let Some(ctx) = WAKE.get() {
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
        }
    );

    pub(super) fn listen(ctx: egui::Context) {
        let _ = WAKE.set(ctx);
        SETUP.call_once(|| {
            let this = Handler::alloc().set_ivars(());
            // SAFETY: NSObject's init on a freshly allocated instance.
            let handler: Retained<Handler> = unsafe { msg_send![super(this), init] };
            // SAFETY: NSAppleEventManager's shared instance, and a handler
            // whose selector takes the event and the reply, as the
            // manager calls it. The manager keeps the handler unretained,
            // so it lives as long as Noura.
            unsafe {
                let manager: *mut AnyObject =
                    msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
                let _: () = msg_send![
                    manager,
                    setEventHandler: &*handler,
                    andSelector: sel!(handleReopen:withReplyEvent:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: REOPEN_APPLICATION,
                ];
            }
            std::mem::forget(handler);
        });
    }
}
