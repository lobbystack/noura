//! Pasting into the app you're typing in, and putting back what the
//! clipboard held.
//!
//! On macOS the text goes on the pasteboard as a promise, so the
//! pasteboard tells Noura when an app reads it, and Noura restores the
//! clipboard only after the app you pasted into read it, as Handy's
//! receipt-sequenced paste does (`src-tauri/src/paste_tx`). On Windows
//! the text goes on the clipboard as a delayed-render promise, whose
//! `WM_RENDERFORMAT` is the receipt, as Handy's Windows paste does. Linux
//! restores after a short wait. On Linux, Noura presses Shift+Insert,
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
    //! The text goes on the clipboard as a delayed-render promise, owned
    //! by a hidden message-only window, as Handy's
    //! `src-tauri/src/paste_tx/windows.rs` does. Windows sends the window
    //! `WM_RENDERFORMAT` when an app reads the text, which is the read
    //! receipt. Once the reads go quiet, Noura puts back every format the
    //! clipboard held, unless someone copied something meanwhile.
    //!
    //! Clipboard ownership and delayed rendering belong to one thread with
    //! a message loop, so each paste runs its window on a thread of its
    //! own. The calling thread presses the keys once the text is on the
    //! clipboard, then waits for that thread to restore it.

    use std::sync::mpsc::{Sender, channel};
    use std::sync::{Arc, Mutex, Once};
    use std::time::Instant;

    use windows::Win32::Foundation::{
        ERROR_SUCCESS, GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, SetLastError,
        WPARAM,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardOwner,
        GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
    };
    use windows::Win32::System::Ole::{
        CF_BITMAP, CF_DSPBITMAP, CF_DSPENHMETAFILE, CF_DSPMETAFILEPICT, CF_DSPTEXT, CF_ENHMETAFILE,
        CF_OWNERDISPLAY, CF_PALETTE, CF_UNICODETEXT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CopyImage, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GDI_IMAGE_TYPE, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, HWND_MESSAGE, IMAGE_FLAGS,
        KillTimer, MSG, PostQuitMessage, RegisterClassW, SetTimer, SetWindowLongPtrW,
        WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROYCLIPBOARD, WM_RENDERALLFORMATS, WM_RENDERFORMAT,
        WM_TIMER, WNDCLASSW,
    };
    use windows::core::{PCWSTR, w};

    use super::*;

    const CLASS_NAME: PCWSTR = w!("NouraPasteWindow");
    const TIMER_ID: usize = 1;
    const TIMER_INTERVAL_MS: u32 = 25;
    /// A clipboard format larger than this isn't saved.
    const MAX_FORMAT_BYTES: usize = 64 * 1024 * 1024;
    const IMAGE_BITMAP: GDI_IMAGE_TYPE = GDI_IMAGE_TYPE(0);
    const LR_CREATEDIBSECTION: IMAGE_FLAGS = IMAGE_FLAGS(0x2000);

    /// One paste at a time owns the clipboard.
    static PASTING: Mutex<()> = Mutex::new(());

    struct SavedFormat {
        format: u32,
        data: Vec<u8>,
    }

    /// What the window and the calling thread share.
    struct Shared {
        transaction: Mutex<Transaction>,
        text: String,
        saved: Mutex<Vec<SavedFormat>>,
        /// A copy of the clipboard's bitmap, as a raw `HBITMAP`.
        bitmap: Mutex<Option<usize>>,
        /// The clipboard's sequence number once the promise is on it.
        sequence: Mutex<u32>,
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    /// # Safety
    ///
    /// `hwnd` is a paste window, whose user data is null or an
    /// `Arc<Shared>` the window owns.
    unsafe fn shared_of(hwnd: HWND) -> *const Shared {
        // SAFETY: see above.
        unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared }
    }

    /// Put the text on the clipboard, which must be open.
    unsafe fn render_text(shared: &Shared) {
        let text = wide(&shared.text);
        // SAFETY: a moveable block the size of the text, filled while
        // locked, which the clipboard owns once SetClipboardData succeeds.
        unsafe {
            let Ok(block) = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2) else {
                return;
            };
            let target = GlobalLock(block).cast::<u16>();
            if target.is_null() {
                let _ = GlobalFree(Some(block));
                return;
            }
            std::ptr::copy_nonoverlapping(text.as_ptr(), target, text.len());
            let _ = GlobalUnlock(block);
            if SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(block.0))).is_err() {
                let _ = GlobalFree(Some(block));
            }
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: this procedure belongs to paste windows only.
        let shared = unsafe { shared_of(hwnd) };
        // SAFETY: the window holds its Arc until it's destroyed.
        let shared = unsafe { shared.as_ref() };
        match message {
            // An app reads the text: the receipt.
            WM_RENDERFORMAT => {
                if let Some(shared) = shared {
                    if let Ok(mut transaction) = shared.transaction.lock() {
                        transaction.reads.push(Instant::now());
                    }
                    if wparam.0 as u32 == u32::from(CF_UNICODETEXT.0) {
                        // SAFETY: Windows opened the clipboard for us.
                        unsafe { render_text(shared) };
                    }
                }
                LRESULT(0)
            }
            // The window goes while its promise is on the clipboard: no
            // app read it, so no receipt. Windows doesn't open the
            // clipboard here.
            WM_RENDERALLFORMATS => {
                if let Some(shared) = shared {
                    // SAFETY: the clipboard opens and closes on this thread.
                    unsafe {
                        if OpenClipboard(Some(hwnd)).is_ok() {
                            if GetClipboardOwner().is_ok_and(|owner| owner == hwnd) {
                                render_text(shared);
                            }
                            let _ = CloseClipboard();
                        }
                    }
                }
                LRESULT(0)
            }
            WM_DESTROYCLIPBOARD => {
                if let Some(shared) = shared
                    && let Ok(mut transaction) = shared.transaction.lock()
                {
                    transaction.ownership_lost = true;
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if let Some(shared) = shared {
                    tick(shared);
                }
                LRESULT(0)
            }
            // SAFETY: the default procedure for any other message.
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    fn register_class(instance: HINSTANCE) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            // SAFETY: a class whose procedure and name are static.
            unsafe { RegisterClassW(&class) };
        });
    }

    /// Put back what the clipboard held. Any thread may call it.
    unsafe fn restore(shared: &Shared) {
        // SAFETY: each block is allocated, filled and handed to the
        // clipboard, or freed when it refuses it.
        unsafe {
            if OpenClipboard(None).is_err() {
                tracing::warn!("Noura couldn't open the clipboard to restore it");
                return;
            }
            let _ = EmptyClipboard();
            if let Ok(saved) = shared.saved.lock() {
                for format in saved.iter().filter(|format| !format.data.is_empty()) {
                    let Ok(block) = GlobalAlloc(GMEM_MOVEABLE, format.data.len()) else {
                        continue;
                    };
                    let target = GlobalLock(block).cast::<u8>();
                    if target.is_null() {
                        let _ = GlobalFree(Some(block));
                        continue;
                    }
                    std::ptr::copy_nonoverlapping(format.data.as_ptr(), target, format.data.len());
                    let _ = GlobalUnlock(block);
                    if SetClipboardData(format.format, Some(HANDLE(block.0))).is_err() {
                        let _ = GlobalFree(Some(block));
                    }
                }
            }
            if let Ok(mut bitmap) = shared.bitmap.lock()
                && let Some(raw) = bitmap.take()
            {
                let _ = SetClipboardData(u32::from(CF_BITMAP.0), Some(HANDLE(raw as *mut _)));
            }
            let _ = CloseClipboard();
        }
    }

    /// Save every format the clipboard holds that is plain memory, and a
    /// copy of its bitmap.
    unsafe fn save(hwnd: HWND, shared: &Shared) -> Result<()> {
        // SAFETY: the clipboard opens on this thread, and each handle is
        // read while it's open.
        unsafe {
            OpenClipboard(Some(hwnd)).map_err(|_| clipboard_failed())?;
            let skipped = [
                CF_ENHMETAFILE,
                CF_DSPENHMETAFILE,
                CF_DSPBITMAP,
                CF_DSPMETAFILEPICT,
                CF_DSPTEXT,
                CF_OWNERDISPLAY,
                CF_PALETTE,
            ]
            .map(|format| u32::from(format.0));
            let mut formats = Vec::new();
            let mut format = 0;
            loop {
                format = EnumClipboardFormats(format);
                if format == 0 {
                    break;
                }
                if format == u32::from(CF_BITMAP.0) {
                    // A GDI object, not memory: copy the image instead.
                    if let Ok(handle) = GetClipboardData(format)
                        && let Ok(copy) = CopyImage(handle, IMAGE_BITMAP, 0, 0, LR_CREATEDIBSECTION)
                        && let Ok(mut bitmap) = shared.bitmap.lock()
                    {
                        *bitmap = Some(copy.0 as usize);
                    }
                    continue;
                }
                if skipped.contains(&format) {
                    continue;
                }
                let Ok(handle) = GetClipboardData(format) else {
                    continue;
                };
                let block = HGLOBAL(handle.0);
                let size = GlobalSize(block);
                if size == 0 || size > MAX_FORMAT_BYTES {
                    continue;
                }
                let source = GlobalLock(block).cast::<u8>();
                if source.is_null() {
                    continue;
                }
                let data = std::slice::from_raw_parts(source, size).to_vec();
                let _ = GlobalUnlock(block);
                formats.push(SavedFormat { format, data });
            }
            let _ = CloseClipboard();
            if let Ok(mut saved) = shared.saved.lock() {
                *saved = formats;
            }
            Ok(())
        }
    }

    /// Put the promise on the clipboard, with the formats that keep the
    /// text out of clipboard history, the cloud clipboard and monitors,
    /// as Chrome does for Incognito copies. Returns the clipboard's new
    /// sequence number.
    unsafe fn publish(hwnd: HWND) -> Result<u32> {
        // SAFETY: the clipboard opens and closes on this thread, on every
        // path.
        unsafe {
            OpenClipboard(Some(hwnd)).map_err(|_| clipboard_failed())?;
            let published = publish_formats();
            let closed = CloseClipboard();
            published?;
            closed.map_err(|_| clipboard_failed())?;
            Ok(GetClipboardSequenceNumber())
        }
    }

    unsafe fn publish_formats() -> Result<()> {
        // SAFETY: called with the clipboard open; each marker block is
        // handed to the clipboard or freed.
        unsafe {
            EmptyClipboard().map_err(|_| clipboard_failed())?;
            for (name, value) in [
                ("ExcludeClipboardContentFromMonitorProcessing", 1u32),
                ("CanIncludeInClipboardHistory", 0u32),
                ("CanUploadToCloudClipboard", 0u32),
            ] {
                let name = wide(name);
                let format = RegisterClipboardFormatW(PCWSTR(name.as_ptr()));
                if format == 0 {
                    continue;
                }
                let Ok(block) = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>()) else {
                    continue;
                };
                let target = GlobalLock(block).cast::<u32>();
                if target.is_null() {
                    let _ = GlobalFree(Some(block));
                    continue;
                }
                *target = value;
                let _ = GlobalUnlock(block);
                if SetClipboardData(format, Some(HANDLE(block.0))).is_err() {
                    let _ = GlobalFree(Some(block));
                }
            }
            // A null handle is the promise. SetClipboardData returns the
            // handle it got, so success is null too and reads as an error:
            // only a thread error set by this call is a failure.
            SetLastError(ERROR_SUCCESS);
            if let Err(error) = SetClipboardData(u32::from(CF_UNICODETEXT.0), None)
                && error.code().is_err()
            {
                return Err(clipboard_failed());
            }
            Ok(())
        }
    }

    /// Restore once the transaction is done, then end the message loop.
    fn tick(shared: &Shared) {
        let (done, lost) = match shared.transaction.lock() {
            Ok(transaction) => (transaction.done(Instant::now()), transaction.ownership_lost),
            Err(_) => (true, true),
        };
        if !done {
            return;
        }
        let sequence = shared.sequence.lock().map_or(0, |sequence| *sequence);
        // SAFETY: reading the sequence number has no preconditions.
        let ours = !lost && unsafe { GetClipboardSequenceNumber() } == sequence;
        // A copy made meanwhile wins.
        if ours {
            // SAFETY: restore opens the clipboard itself.
            unsafe { restore(shared) };
        }
        // SAFETY: ends this thread's message loop.
        unsafe { PostQuitMessage(0) };
    }

    /// The window's thread: save the clipboard, publish the promise, say
    /// so on `ready`, then answer reads until the transaction is done.
    fn window_thread(shared: Arc<Shared>, ready: Sender<Result<()>>) {
        // SAFETY: the window and its user data live on this thread; the
        // Arc it holds comes back and drops after the window goes.
        unsafe {
            let Ok(module) = GetModuleHandleW(PCWSTR::null()) else {
                let _ = ready.send(Err(clipboard_failed()));
                return;
            };
            let instance = HINSTANCE(module.0);
            register_class(instance);
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS_NAME,
                w!("NouraPaste"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                None,
            ) else {
                let _ = ready.send(Err(clipboard_failed()));
                return;
            };
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::into_raw(shared.clone()) as isize);
            let destroy = |hwnd: HWND| {
                let owned = shared_of(hwnd);
                let _ = DestroyWindow(hwnd);
                if !owned.is_null() {
                    drop(Arc::from_raw(owned));
                }
            };
            let published = save(hwnd, &shared).and_then(|()| {
                publish(hwnd).inspect_err(|_| {
                    // Publishing may have emptied the clipboard.
                    restore(&shared);
                })
            });
            let sequence = match published {
                Ok(sequence) => sequence,
                Err(error) => {
                    destroy(hwnd);
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            if let Ok(mut known) = shared.sequence.lock() {
                *known = sequence;
            }
            if let Ok(mut transaction) = shared.transaction.lock() {
                transaction.published_at = Instant::now();
            }
            let _ = SetTimer(Some(hwnd), TIMER_ID, TIMER_INTERVAL_MS, None);
            let _ = ready.send(Ok(()));
            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = DispatchMessageW(&message);
            }
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            destroy(hwnd);
        }
    }

    pub(super) fn paste(text: &str) -> Result<()> {
        let _pasting = PASTING.lock().unwrap_or_else(|error| error.into_inner());
        let shared = Arc::new(Shared {
            transaction: Mutex::new(Transaction::new(Instant::now())),
            text: text.to_owned(),
            saved: Mutex::new(Vec::new()),
            bitmap: Mutex::new(None),
            sequence: Mutex::new(0),
        });
        let (ready, published) = channel();
        let window = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("noura-paste".into())
                .spawn(move || window_thread(shared, ready))
                .map_err(|_| clipboard_failed())?
        };
        match published.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = window.join();
                return Err(error);
            }
            Err(_) => return Err(clipboard_failed()),
        }
        // Mark the press first: a fast app reads while the keys are down.
        if let Ok(mut transaction) = shared.transaction.lock() {
            transaction.pressed_at = Some(Instant::now());
        }
        let pressed = chord(enigo::Key::Control, enigo::Key::Unicode('v'));
        if pressed.is_err()
            && let Ok(mut transaction) = shared.transaction.lock()
        {
            transaction.press_failed = true;
        }
        let _ = window.join();
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
