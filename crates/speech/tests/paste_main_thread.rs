//! The real paste and copy, called from a thread other than the main
//! one, as a plugin's worker thread calls them, while the main thread runs
//! AppKit's event loop with a text field in a key window, as Noura's does.
//! macOS traps when its input source calls run off the main thread, so
//! this test crashed with SIGTRAP before pastes hopped to the main thread.
//!
//! The test comes forward and pastes into its own text view. If it can't
//! take the keyboard, it presses ⌘V with an empty string instead, so no
//! text reaches another app. It puts back what the clipboard held, and it
//! runs only with `NOURA_PASTE_TEST=1`:
//! `NOURA_PASTE_TEST=1 cargo test -p speech --test paste_main_thread`.
//! The process that runs it needs Accessibility permission to press keys.

#[cfg(target_os = "macos")]
fn main() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use speech::paste::{Paster, SystemPaste};

    if std::env::var_os("NOURA_PASTE_TEST").is_none_or(|value| value != "1") {
        eprintln!("Set NOURA_PASTE_TEST=1 to paste and press keys in this test's own window.");
        return;
    }
    let mtm = MainThreadMarker::new().expect("the test runs on the main thread");
    // As winit starts Noura's event loop.
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    edit_menu(&app, mtm);
    let (window, text) = text_window(mtm);
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(1) {
        pump(&app);
    }
    let ours = app.isActive() && window.isKeyWindow();
    if !ours {
        eprintln!("The test didn't get the keyboard; it pastes nothing.");
    }
    let saved = arboard::Clipboard::new().unwrap().get_text().ok();

    let (done, finished) = mpsc::channel();
    let (pasting, typing) = if ours {
        ("Pasted", " and typed")
    } else {
        ("", "")
    };
    std::thread::spawn(move || {
        let pasted = SystemPaste.paste(pasting, false);
        let typed = SystemPaste.paste(typing, true);
        let restored = arboard::Clipboard::new().unwrap().get_text().ok();
        let copied = SystemPaste.copy("Copied");
        let _ = done.send((pasted, typed, restored, copied));
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let (pasted, typed, restored, copied) = loop {
        if let Ok(results) = finished.try_recv() {
            break results;
        }
        assert!(Instant::now() < deadline, "the paste never finished");
        pump(&app);
    };
    // Let the text view take the last typed keys.
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(500) {
        pump(&app);
    }
    let copied_text = arboard::Clipboard::new().unwrap().get_text().ok();
    // On the main thread itself, a copy runs at once.
    let inline = SystemPaste.copy("Copied here");
    let inline_text = arboard::Clipboard::new().unwrap().get_text().ok();
    let shown = text.string().to_string();

    window.close();
    let mut clipboard = arboard::Clipboard::new().unwrap();
    match &saved {
        Some(saved) => clipboard.set_text(saved).unwrap(),
        None => clipboard.clear().unwrap(),
    }
    println!("paste from another thread: {pasted:?}");
    println!("typing from another thread: {typed:?}");
    println!("copy from another thread: {copied:?}");
    println!("copy on the main thread: {inline:?}");
    println!("the text view holds {shown:?}");
    assert!(pasted.is_ok() && typed.is_ok() && copied.is_ok() && inline.is_ok());
    assert_eq!(
        restored, saved,
        "the paste put back what the clipboard held"
    );
    assert_eq!(copied_text.as_deref(), Some("Copied"));
    assert_eq!(inline_text.as_deref(), Some("Copied here"));
    if ours {
        assert_eq!(shown, "Pasted and typed");
    }
}

/// Handle AppKit's events for a moment, as Noura's event loop does.
#[cfg(target_os = "macos")]
fn pump(app: &objc2_app_kit::NSApplication) {
    use objc2_app_kit::NSEventMask;
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

    let until = NSDate::dateWithTimeIntervalSinceNow(0.02);
    // SAFETY: NSDefaultRunLoopMode is a framework constant.
    let mode = unsafe { NSDefaultRunLoopMode };
    while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
        NSEventMask::Any,
        Some(&until),
        mode,
        true,
    ) {
        app.sendEvent(&event);
    }
}

/// A menu bar whose Paste item takes ⌘V, as every app's Edit menu does:
/// AppKit hands ⌘V to the text view through it.
#[cfg(target_os = "macos")]
fn edit_menu(app: &objc2_app_kit::NSApplication, mtm: objc2::MainThreadMarker) {
    use objc2::{MainThreadOnly, sel};
    use objc2_app_kit::{NSMenu, NSMenuItem};
    use objc2_foundation::ns_string;

    let bar = NSMenu::new(mtm);
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Edit"));
    // SAFETY: paste: is NSText's action, which the text view answers.
    let paste = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            ns_string!("Paste"),
            Some(sel!(paste:)),
            ns_string!("v"),
        )
    };
    edit.addItem(&paste);
    let holder = NSMenuItem::new(mtm);
    holder.setSubmenu(Some(&edit));
    bar.addItem(&holder);
    app.setMainMenu(Some(&bar));
}

/// A key window holding a text view with the keyboard focus, as Noura's
/// window holds a text field, so the system's text input is live.
#[cfg(target_os = "macos")]
fn text_window(
    mtm: objc2::MainThreadMarker,
) -> (
    objc2::rc::Retained<objc2_app_kit::NSWindow>,
    objc2::rc::Retained<objc2_app_kit::NSTextView>,
) {
    use objc2::MainThreadOnly;
    use objc2_app_kit::{NSBackingStoreType, NSTextView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let frame = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(320.0, 120.0));
    // SAFETY: a plain titled window. `close` doesn't release it, since
    // the caller holds it.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the caller keeps the window, so closing it frees nothing.
    unsafe { window.setReleasedWhenClosed(false) };
    let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
    window.setContentView(Some(&text));
    window.makeKeyAndOrderFront(None);
    window.makeFirstResponder(Some(&text));
    (window, text)
}

#[cfg(not(target_os = "macos"))]
fn main() {}
