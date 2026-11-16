//! Pasting into the app you're typing in, and putting back what the
//! clipboard held.
//!
//! On macOS the text goes on the pasteboard as a promise, so the
//! pasteboard tells Noura when an app reads it, and Noura restores the
//! clipboard only after the app you pasted into read it, as Handy's
//! receipt-sequenced paste does (`src-tauri/src/paste_tx`). Windows and
//! Linux restore after a short wait. On Linux, Noura presses Shift+Insert,
//! which pastes in terminals too, and puts back the primary selection.
//! Under Wayland it presses the keys with `wtype`, `dotool` or `ydotool`,
//! whichever is installed.

use std::time::{Duration, Instant};

use crate::{Result, SpeechError};

/// How long the pasted text may stay on the clipboard, at most.
pub const RESTORE_TIMEOUT: Duration = Duration::from_secs(8);
/// After the last read, how long Noura waits before restoring, for apps
/// that read the clipboard twice per paste.
pub const QUIET_PERIOD: Duration = Duration::from_millis(200);
/// When the keys couldn't be pressed, nobody reads the text.
pub const FAILED_PRESS_TIMEOUT: Duration = Duration::from_millis(500);
/// Where Noura can't tell when an app read the clipboard.
pub const FIXED_WAIT: Duration = Duration::from_millis(500);
/// How long the paste keys stay down.
const HOLD: Duration = Duration::from_millis(100);

/// Pastes and copies. The kernel uses [`SystemPaste`]; tests a fake.
pub trait Paster: Send + Sync {
    /// Paste `text` into the focused app, through the clipboard or, with
    /// `typing`, key by key. Returns once the clipboard holds what it held
    /// before.
    fn paste(&self, text: &str, typing: bool) -> Result<()>;

    /// Put `text` on the clipboard and leave it there.
    fn copy(&self, text: &str) -> Result<()>;
}

/// One paste through the clipboard, as the pasteboard reports reads.
#[derive(Debug)]
pub struct Transaction {
    pub published_at: Instant,
    /// When the paste keys went down. Only reads after it count.
    pub pressed_at: Option<Instant>,
    pub press_failed: bool,
    pub reads: Vec<Instant>,
    /// Someone else put something on the clipboard.
    pub ownership_lost: bool,
}

impl Transaction {
    pub fn new(now: Instant) -> Self {
        Self {
            published_at: now,
            pressed_at: None,
            press_failed: false,
            reads: Vec::new(),
            ownership_lost: false,
        }
    }

    /// Whether to restore the clipboard now.
    pub fn done(&self, now: Instant) -> bool {
        if self.ownership_lost {
            return true;
        }
        let read = self
            .pressed_at
            .and_then(|pressed| self.reads.iter().rev().find(|read| **read >= pressed));
        if read.is_some_and(|read| now.duration_since(*read) >= QUIET_PERIOD) {
            return true;
        }
        let limit = if self.press_failed {
            FAILED_PRESS_TIMEOUT
        } else {
            RESTORE_TIMEOUT
        };
        now.duration_since(self.published_at) >= limit
    }
}

fn clipboard_failed() -> SpeechError {
    SpeechError::failed("clipboard_failed", "Noura couldn't use the clipboard.")
}

fn keys_failed() -> SpeechError {
    SpeechError::failed(
        "paste_keys_failed",
        "Noura couldn't press keys in the other app. Allow Noura in your system's \
         accessibility settings.",
    )
}

/// The computer's clipboard and keyboard.
#[derive(Default)]
pub struct SystemPaste;

impl Paster for SystemPaste {
    fn paste(&self, text: &str, typing: bool) -> Result<()> {
        if typing {
            return type_text(text);
        }
        platform::paste(text)
    }

    fn copy(&self, text: &str) -> Result<()> {
        let mut clipboard = arboard::Clipboard::new().map_err(|_| clipboard_failed())?;
        clipboard.set_text(text).map_err(|_| clipboard_failed())
    }
}

fn enigo() -> Result<enigo::Enigo> {
    enigo::Enigo::new(&enigo::Settings::default()).map_err(|error| {
        tracing::warn!(%error, "enigo didn't start");
        keys_failed()
    })
}

fn type_text(text: &str) -> Result<()> {
    use enigo::Keyboard;
    #[cfg(target_os = "linux")]
    if wayland() {
        return wayland_type(text);
    }
    enigo()?.text(text).map_err(|_| keys_failed())
}

/// Press `modifier` and `key` together.
fn chord(modifier: enigo::Key, key: enigo::Key) -> Result<()> {
    use enigo::{Direction, Keyboard};
    let mut enigo = enigo()?;
    enigo
        .key(modifier, Direction::Press)
        .map_err(|_| keys_failed())?;
    let pressed = enigo.key(key, Direction::Click);
    std::thread::sleep(HOLD);
    let released = enigo.key(modifier, Direction::Release);
    pressed.and(released).map_err(|_| keys_failed())
}

#[cfg(target_os = "linux")]
fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Run the first of `tools` that's installed and succeeds.
#[cfg(target_os = "linux")]
fn run_first(tools: &[(&str, &[&str], Option<&str>)]) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (program, args, input) in tools {
        let child = Command::new(program)
            .args(*args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            continue;
        };
        if let (Some(input), Some(stdin)) = (input, child.stdin.as_mut()) {
            let _ = stdin.write_all(input.as_bytes());
        }
        drop(child.stdin.take());
        if child.wait().is_ok_and(|status| status.success()) {
            return Ok(());
        }
    }
    Err(SpeechError::failed(
        "paste_keys_failed",
        "Noura needs wtype, dotool or ydotool to paste under Wayland. Install one.",
    ))
}

#[cfg(target_os = "linux")]
fn wayland_type(text: &str) -> Result<()> {
    let dotool = format!("type {text}\n");
    run_first(&[
        ("wtype", &[text], None),
        ("dotool", &[], Some(&dotool)),
        ("ydotool", &["type", text], None),
    ])
}

#[cfg(target_os = "linux")]
mod platform {
    use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

    use super::*;

    fn shift_insert() -> Result<()> {
        if wayland() {
            return run_first(&[
                (
                    "wtype",
                    &["-M", "shift", "-k", "Insert", "-m", "shift"],
                    None,
                ),
                ("dotool", &[], Some("key shift+Insert\n")),
                ("ydotool", &["key", "42:1", "110:1", "110:0", "42:0"], None),
            ]);
        }
        chord(enigo::Key::Shift, enigo::Key::Insert)
    }

    pub(super) fn paste(text: &str) -> Result<()> {
        let mut clipboard = arboard::Clipboard::new().map_err(|_| clipboard_failed())?;
        let saved = clipboard.get_text().ok();
        let saved_primary = clipboard
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text()
            .ok();
        clipboard.set_text(text).map_err(|_| clipboard_failed())?;
        let _ = clipboard
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text);
        let pressed = shift_insert();
        std::thread::sleep(FIXED_WAIT);
        // Put back only what Noura replaced: a copy made meanwhile wins.
        if clipboard.get_text().ok().as_deref() == Some(text) {
            match saved {
                Some(saved) => {
                    let _ = clipboard.set_text(saved);
                }
                None => {
                    let _ = clipboard.clear();
                }
            }
        }
        if let Some(saved) = saved_primary {
            let _ = clipboard
                .set()
                .clipboard(LinuxClipboardKind::Primary)
                .text(saved);
        }
        pressed
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;

    pub(super) fn paste(text: &str) -> Result<()> {
        let mut clipboard = arboard::Clipboard::new().map_err(|_| clipboard_failed())?;
        let saved = clipboard.get_text().ok();
        clipboard.set_text(text).map_err(|_| clipboard_failed())?;
        let pressed = chord(enigo::Key::Control, enigo::Key::Unicode('v'));
        std::thread::sleep(FIXED_WAIT);
        if clipboard.get_text().ok().as_deref() == Some(text) {
            match saved {
                Some(saved) => {
                    let _ = clipboard.set_text(saved);
                }
                None => {
                    let _ = clipboard.clear();
                }
            }
        }
        pressed
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::{Arc, Mutex};

    use objc2::rc::Retained;
    use objc2::{AnyThread, DefinedClass, define_class, msg_send};
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::{NSArray, NSInteger, NSObject, NSString};

    use super::*;

    /// Types that clipboard managers such as Maccy skip, so fewer apps
    /// read the text before the one you pasted into.
    const CONCEALED: [&str; 3] = [
        "org.nspasteboard.TransientType",
        "org.nspasteboard.ConcealedType",
        "org.nspasteboard.AutoGeneratedType",
    ];

    pub struct ProviderIvars {
        transaction: Arc<Mutex<Transaction>>,
        text: String,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and the ivars
        // are plain Rust values behind a Mutex.
        #[unsafe(super(NSObject))]
        #[name = "NouraPasteProvider"]
        #[ivars = ProviderIvars]
        pub struct PasteProvider;

        impl PasteProvider {
            /// The pasteboard asks for the promised text: an app read it.
            #[unsafe(method(pasteboard:provideDataForType:))]
            fn provide(&self, pasteboard: &NSPasteboard, kind: &NSString) {
                let ivars = self.ivars();
                // SAFETY: NSPasteboardTypeString is a read-only framework
                // static.
                let text = unsafe { kind.isEqualToString(NSPasteboardTypeString) };
                if text && let Ok(mut transaction) = ivars.transaction.lock() {
                    transaction.reads.push(Instant::now());
                }
                let payload = NSString::from_str(if text { &ivars.text } else { "" });
                let _ = pasteboard.setString_forType(&payload, kind);
            }

            #[unsafe(method(pasteboardChangedOwner:))]
            fn changed_owner(&self, _pasteboard: &NSPasteboard) {
                if let Ok(mut transaction) = self.ivars().transaction.lock() {
                    transaction.ownership_lost = true;
                }
            }
        }
    );

    impl PasteProvider {
        fn new(transaction: Arc<Mutex<Transaction>>, text: String) -> Retained<Self> {
            let this = Self::alloc().set_ivars(ProviderIvars { transaction, text });
            // SAFETY: NSObject's init on a freshly allocated instance.
            unsafe { msg_send![super(this), init] }
        }
    }

    pub(super) fn paste(text: &str) -> Result<()> {
        let mut clipboard = arboard::Clipboard::new().map_err(|_| clipboard_failed())?;
        let saved = clipboard.get_text().ok();
        let transaction = Arc::new(Mutex::new(Transaction::new(Instant::now())));
        let provider = PasteProvider::new(transaction.clone(), text.to_owned());
        let pasteboard = NSPasteboard::generalPasteboard();
        let mut kinds: Vec<Retained<NSString>> = vec![NSString::from_str("public.utf8-plain-text")];
        kinds.extend(CONCEALED.iter().map(|kind| NSString::from_str(kind)));
        let kinds = NSArray::from_retained_slice(&kinds);
        // SAFETY: declareTypes:owner: with an array of type strings and an
        // object that answers the owner methods; it returns the new
        // changeCount.
        let change: NSInteger =
            unsafe { msg_send![&*pasteboard, declareTypes: &*kinds, owner: &*provider] };
        if change <= 0 {
            return Err(clipboard_failed());
        }
        if let Ok(mut transaction) = transaction.lock() {
            transaction.pressed_at = Some(Instant::now());
        }
        let pressed = chord(enigo::Key::Meta, enigo::Key::Unicode('v'));
        if pressed.is_err()
            && let Ok(mut transaction) = transaction.lock()
        {
            transaction.press_failed = true;
        }
        loop {
            let done = transaction
                .lock()
                .map_or(true, |transaction| transaction.done(Instant::now()));
            if done {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let lost = transaction
            .lock()
            .map_or(true, |transaction| transaction.ownership_lost);
        // Restore only while the promise is still on the pasteboard: a
        // copy made meanwhile wins.
        if !lost && pasteboard.changeCount() == change {
            match saved {
                Some(saved) => {
                    let _ = clipboard.set_text(saved);
                }
                None => {
                    let _ = clipboard.clear();
                }
            }
        }
        drop(provider);
        pressed
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    use super::*;

    pub(super) fn paste(_text: &str) -> Result<()> {
        Err(keys_failed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clipboard_comes_back_after_the_last_read_or_a_timeout() {
        let start = Instant::now();
        let mut transaction = Transaction::new(start);
        assert!(!transaction.done(start));
        // A read before the keys went down is a clipboard manager.
        transaction.reads.push(start);
        transaction.pressed_at = Some(start + Duration::from_millis(5));
        assert!(!transaction.done(start + Duration::from_secs(1)));
        transaction.reads.push(start + Duration::from_millis(50));
        assert!(!transaction.done(start + Duration::from_millis(100)));
        assert!(transaction.done(start + Duration::from_millis(260)));

        let mut unread = Transaction::new(start);
        unread.pressed_at = Some(start);
        assert!(!unread.done(start + Duration::from_secs(7)));
        assert!(unread.done(start + RESTORE_TIMEOUT));
        unread.press_failed = true;
        assert!(unread.done(start + FAILED_PRESS_TIMEOUT));
        let mut lost = Transaction::new(start);
        lost.ownership_lost = true;
        assert!(lost.done(start));
    }
}
