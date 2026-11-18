//! Recording the microphone and the sound other apps play, each as its own
//! channel of 16 kHz mono audio.
//!
//! The microphone goes through `handy-recorder`, which opens the device at
//! the system's format, resamples and reports a device that goes away.
//! When the microphone disappears mid-recording, as when you unplug
//! headphones, Noura opens the system's default one and goes on, as
//! anarlog's `audio-actual` does.
//!
//! System audio comes from a Core Audio process tap on macOS 14.4 or later
//! (`cidre`), a WASAPI loopback stream on Windows that follows the output
//! device you switch to, and the PulseAudio monitor of the default output
//! on Linux, which PipeWire serves through `pipewire-pulse`.

use std::sync::Arc;

use crate::{Result, SpeechError};

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod in_use;
mod mic;
mod resample;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod system;

pub use resample::MonoResampler;

/// What a capture hands its audio to: 16 kHz mono samples, in order, on a
/// thread of its own.
pub type AudioSink = Arc<dyn Fn(&[f32]) + Send + Sync>;

/// What a capture calls once when it fails for good, with why in words for
/// you.
pub type FailureSink = Arc<dyn Fn(SpeechError) + Send + Sync>;

/// What to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    Microphone,
    SystemAudio,
}

/// A microphone this computer has now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Microphone {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// A running capture. Dropping it stops recording.
pub trait Capture: Send {}

/// The computer's audio devices.
pub trait AudioBackend: Send + Sync {
    fn microphones(&self) -> Result<Vec<Microphone>>;

    /// Start recording `source`, from the microphone `microphone` or the
    /// default one. Fails when the device can't open.
    fn open(
        &self,
        source: Source,
        microphone: Option<&str>,
        sink: AudioSink,
        failed: FailureSink,
    ) -> Result<Box<dyn Capture>>;

    /// Whether some app has the default microphone on now, or `None` where
    /// Noura can't tell.
    fn microphone_in_use(&self) -> Option<bool> {
        None
    }
}

/// The real devices.
#[derive(Default)]
pub struct SystemAudio;

impl AudioBackend for SystemAudio {
    fn microphones(&self) -> Result<Vec<Microphone>> {
        mic::microphones()
    }

    fn open(
        &self,
        source: Source,
        microphone: Option<&str>,
        sink: AudioSink,
        failed: FailureSink,
    ) -> Result<Box<dyn Capture>> {
        match source {
            Source::Microphone => mic::open(microphone.map(str::to_owned), sink, failed),
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            Source::SystemAudio => system::open(sink, failed),
            #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
            Source::SystemAudio => Err(SpeechError::failed(
                "system_audio_unavailable",
                "Noura can't record system audio on this computer.",
            )),
        }
    }

    fn microphone_in_use(&self) -> Option<bool> {
        #[cfg(target_os = "macos")]
        {
            system::microphone_in_use()
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            in_use::microphone_in_use()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            None
        }
    }
}

/// Devices for tests: [`FakeAudio::feed`] hands audio to the captures that
/// record a source, and [`FakeAudio::fail`] fails them.
#[derive(Default)]
pub struct FakeAudio {
    open: parking_lot::Mutex<Vec<(u64, Source, AudioSink, FailureSink)>>,
    next: std::sync::atomic::AtomicU64,
    /// Sources that refuse to open.
    pub broken: parking_lot::Mutex<Vec<Source>>,
    /// What `microphone_in_use` answers.
    pub in_use: parking_lot::Mutex<Option<bool>>,
}

struct FakeCapture {
    id: u64,
    audio: std::sync::Weak<FakeAudio>,
}

impl Capture for FakeCapture {}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        if let Some(audio) = self.audio.upgrade() {
            audio.open.lock().retain(|(id, ..)| *id != self.id);
        }
    }
}

impl FakeAudio {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Hand `samples` to every capture of `source`.
    pub fn feed(&self, source: Source, samples: &[f32]) {
        let sinks: Vec<AudioSink> = self
            .open
            .lock()
            .iter()
            .filter(|(_, open, ..)| *open == source)
            .map(|(_, _, sink, _)| sink.clone())
            .collect();
        for sink in sinks {
            sink(samples);
        }
    }

    /// Fail every capture of `source`.
    pub fn fail(&self, source: Source) {
        let failures: Vec<FailureSink> = self
            .open
            .lock()
            .iter()
            .filter(|(_, open, ..)| *open == source)
            .map(|(.., failed)| failed.clone())
            .collect();
        for failed in failures {
            failed(SpeechError::failed(
                "device_lost",
                "The microphone went away.",
            ));
        }
    }

    /// How many captures are recording now.
    pub fn open_count(&self) -> usize {
        self.open.lock().len()
    }
}

/// [`AudioBackend`] for an `Arc<FakeAudio>`, so a test keeps a handle.
pub struct FakeBackend(pub Arc<FakeAudio>);

impl AudioBackend for FakeBackend {
    fn microphones(&self) -> Result<Vec<Microphone>> {
        Ok(vec![Microphone {
            id: "fake".into(),
            name: "Test microphone".into(),
            is_default: true,
        }])
    }

    fn open(
        &self,
        source: Source,
        _microphone: Option<&str>,
        sink: AudioSink,
        failed: FailureSink,
    ) -> Result<Box<dyn Capture>> {
        if self.0.broken.lock().contains(&source) {
            return Err(SpeechError::failed(
                "device_unavailable",
                "No microphone is connected.",
            ));
        }
        let id = self
            .0
            .next
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.0.open.lock().push((id, source, sink, failed));
        Ok(Box::new(FakeCapture {
            id,
            audio: Arc::downgrade(&self.0),
        }))
    }

    fn microphone_in_use(&self) -> Option<bool> {
        *self.0.in_use.lock()
    }
}
