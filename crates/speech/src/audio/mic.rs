//! The microphone through `handy-recorder`, which also records the
//! PulseAudio monitor that stands for system audio on Linux.
//!
//! A recorder that fails because its device went away, the default device
//! changed or the stream stalled, opens again on the system's default
//! device a few times before the capture fails for good.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use handy_recorder::{AudioChunk, ErrorKind, Recorder, RecorderConfig, Sink};

use super::{AudioSink, Capture, FailureSink, Microphone};
use crate::{Result, SpeechError};

/// How many times a failed device opens again before the capture fails.
const REOPENS: u32 = 10;
const REOPEN_PAUSE: Duration = Duration::from_millis(500);

pub(super) fn microphones() -> Result<Vec<Microphone>> {
    let devices = handy_recorder::list_input_devices().map_err(|error| refusal(&error))?;
    Ok(devices
        .into_iter()
        .filter(|device| !device.is_monitor)
        .map(|device| Microphone {
            id: device.id,
            name: device.name,
            is_default: device.is_default,
        })
        .collect())
}

/// The PulseAudio monitor of the default output, which records what other
/// apps play.
#[cfg(target_os = "linux")]
pub(super) fn default_monitor() -> Option<String> {
    let devices = handy_recorder::list_input_devices().ok()?;
    let monitors: Vec<_> = devices
        .into_iter()
        .filter(|device| device.is_monitor)
        .collect();
    monitors
        .iter()
        .find(|device| device.is_default)
        .or_else(|| monitors.first())
        .map(|device| device.id.clone())
}

/// Hands each chunk to the capture's sink.
struct Forward(AudioSink);

impl Sink for Forward {
    fn process_chunk(&mut self, chunk: AudioChunk<'_>) {
        (self.0)(&chunk.samples[..chunk.valid_frames * usize::from(chunk.channels)]);
    }
}

enum Event {
    Failed(handy_recorder::Error),
    Stop,
}

struct MicCapture {
    stop: mpsc::Sender<Event>,
}

impl Capture for MicCapture {}

impl Drop for MicCapture {
    fn drop(&mut self) {
        let _ = self.stop.send(Event::Stop);
    }
}

fn start(
    device: Option<String>,
    sink: &AudioSink,
    events: &mpsc::Sender<Event>,
) -> std::result::Result<Recorder<Forward>, handy_recorder::Error> {
    let tell = events.clone();
    let recorder = Recorder::open_with_failure_handler(
        RecorderConfig {
            device,
            ..RecorderConfig::speech()
        },
        move |error| {
            let _ = tell.send(Event::Failed(error));
        },
    )?;
    if let Err(failed) = recorder.start(Forward(sink.clone())) {
        return Err(failed.error);
    }
    Ok(recorder)
}

/// Record the device `device`, or the default one, until the capture
/// drops. When it fails, `reopen` names the device to try next.
pub(super) fn open_with(
    device: Option<String>,
    reopen: fn() -> Option<String>,
    sink: AudioSink,
    failed: FailureSink,
) -> Result<Box<dyn Capture>> {
    let (events, received) = mpsc::channel();
    let first = start(device, &sink, &events).map_err(|error| refusal(&error))?;
    let tell = events.clone();
    thread::Builder::new()
        .name("noura-microphone".into())
        .spawn(move || {
            let mut recorder = Some(first);
            while let Ok(event) = received.recv() {
                let error = match event {
                    Event::Stop => break,
                    Event::Failed(error) => error,
                };
                if let Some(old) = recorder.take() {
                    let _ = old.stop();
                    let _ = old.close();
                }
                if !recoverable(error.kind()) {
                    failed(refusal(&error));
                    break;
                }
                tracing::info!(kind = ?error.kind(), "the microphone went away; opening the default");
                for _ in 0..REOPENS {
                    thread::sleep(REOPEN_PAUSE);
                    if let Ok(next) = start(reopen(), &sink, &tell) {
                        recorder = Some(next);
                        break;
                    }
                }
                if recorder.is_none() {
                    failed(refusal(&error));
                    break;
                }
            }
            if let Some(recorder) = recorder {
                let _ = recorder.stop();
                let _ = recorder.close();
            }
        })
        .map_err(|error| SpeechError::io(&error, "microphone thread"))?;
    Ok(Box::new(MicCapture { stop: events }))
}

pub(super) fn open(
    device: Option<String>,
    sink: AudioSink,
    failed: FailureSink,
) -> Result<Box<dyn Capture>> {
    open_with(device, || None, sink, failed)
}

fn recoverable(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::DeviceLost
            | ErrorKind::StreamInvalidated
            | ErrorKind::Stalled
            | ErrorKind::NoAudio
            | ErrorKind::DeviceUnavailable
    )
}

/// What a device error means, in words for you.
fn refusal(error: &handy_recorder::Error) -> SpeechError {
    tracing::warn!(%error, "the microphone failed");
    match error.kind() {
        ErrorKind::PermissionDenied => SpeechError::failed(
            "microphone_denied",
            "Noura can't use the microphone. Allow it in your system's privacy settings.",
        ),
        ErrorKind::DeviceUnavailable => {
            SpeechError::failed("device_unavailable", "No microphone is connected.")
        }
        ErrorKind::DeviceBusy => SpeechError::failed(
            "device_busy",
            "Another app is using the microphone. Close it and try again.",
        ),
        _ => SpeechError::failed(
            "device_failed",
            "The microphone stopped working. Check it and try again.",
        ),
    }
}
