//! One tracing subscriber for the whole test binary.
//!
//! A process holds one global subscriber, and the tests that read the logs
//! need every thread's lines, the app's background threads included, so a
//! thread-local subscriber can't stand in. This one is installed once and
//! writes each event to every [`Capture`] alive at that moment whose targets
//! enable it. Tests run in parallel, so a capture also holds lines other
//! tests wrote while it was open: check a capture for what must be absent,
//! or for lines only your own test writes.
//!
//! With no capture open, the filter turns every event away before it is
//! formatted.

use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use tracing::Metadata;
use tracing_subscriber::filter::{Targets, dynamic_filter_fn};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Layer, Registry};

enum Out {
    Buffer(Arc<Mutex<Vec<u8>>>),
    Stderr,
}

struct Sink {
    id: u64,
    targets: Targets,
    out: Out,
}

static SINKS: RwLock<Vec<Sink>> = RwLock::new(Vec::new());
static OPEN: AtomicUsize = AtomicUsize::new(0);
static NEXT: AtomicU64 = AtomicU64::new(0);

fn enabled(metadata: &Metadata<'_>) -> bool {
    OPEN.load(Ordering::Acquire) > 0
        && SINKS.read().unwrap().iter().any(|sink| {
            sink.targets
                .would_enable(metadata.target(), metadata.level())
        })
}

fn install() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(FanOut)
            .with_filter(dynamic_filter_fn(|metadata, _| enabled(metadata)));
        tracing::subscriber::set_global_default(Registry::default().with(layer))
            .expect("no other subscriber in this binary");
    });
}

fn open(targets: Targets, out: Out) -> u64 {
    install();
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    SINKS.write().unwrap().push(Sink { id, targets, out });
    OPEN.fetch_add(1, Ordering::Release);
    id
}

fn close(id: u64) {
    SINKS.write().unwrap().retain(|sink| sink.id != id);
    OPEN.fetch_sub(1, Ordering::Release);
}

/// Every line logged at a level and target `targets` enables, from any
/// thread, while this lives.
pub struct Capture {
    id: u64,
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl Capture {
    /// Every target at every level.
    pub fn all() -> Self {
        Self::of(Targets::new().with_default(tracing::Level::TRACE))
    }

    pub fn of(targets: Targets) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let id = open(targets, Out::Buffer(buffer.clone()));
        Self { id, buffer }
    }

    /// What was logged so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).into_owned()
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.buffer.lock().unwrap().clone()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        close(self.id);
    }
}

/// Prints error lines to the output of the test whose thread logged them
/// while this lives.
pub struct PrintErrors(u64);

impl PrintErrors {
    pub fn start() -> Self {
        Self(open(
            Targets::new().with_default(tracing::Level::ERROR),
            Out::Stderr,
        ))
    }
}

impl Drop for PrintErrors {
    fn drop(&mut self) {
        close(self.0);
    }
}

struct FanOut;

/// One formatted event, handed to the sinks that enable it when the
/// formatter drops it.
struct Line {
    target: String,
    level: tracing::Level,
    bytes: Vec<u8>,
}

impl Write for Line {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        for sink in SINKS.read().unwrap().iter() {
            if !sink.targets.would_enable(&self.target, &self.level) {
                continue;
            }
            match &sink.out {
                Out::Buffer(buffer) => buffer.lock().unwrap().extend_from_slice(&self.bytes),
                // `eprint!` reaches the output libtest captures for the
                // test that owns the logging thread.
                Out::Stderr => eprint!("{}", String::from_utf8_lossy(&self.bytes)),
            }
        }
    }
}

impl<'a> MakeWriter<'a> for FanOut {
    type Writer = Line;

    fn make_writer(&'a self) -> Line {
        Line {
            target: String::new(),
            level: tracing::Level::TRACE,
            bytes: Vec::new(),
        }
    }

    fn make_writer_for(&'a self, metadata: &Metadata<'_>) -> Line {
        Line {
            target: metadata.target().to_owned(),
            level: *metadata.level(),
            bytes: Vec::new(),
        }
    }
}
