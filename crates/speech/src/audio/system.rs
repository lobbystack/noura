//! System audio: what other apps play, as the other side of a call.

use super::{AudioSink, Capture, FailureSink};
use crate::Result;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::SpeechError;

#[cfg(target_os = "linux")]
pub(super) fn open(sink: AudioSink, failed: FailureSink) -> Result<Box<dyn Capture>> {
    let Some(monitor) = super::mic::default_monitor() else {
        return Err(crate::SpeechError::failed(
            "system_audio_unavailable",
            "Noura found no sound server to record system audio from.",
        ));
    };
    super::mic::open_with(Some(monitor), super::mic::default_monitor, sink, failed)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct Running {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Capture for Running {}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Run `capture` on a thread of its own until the capture drops, and
/// return once it has started or failed to.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn spawn(
    name: &str,
    capture: impl FnOnce(
        &std::sync::atomic::AtomicBool,
        &dyn Fn(Result<()>),
    ) -> std::result::Result<(), SpeechError>
    + Send
    + 'static,
    failed: FailureSink,
) -> Result<Box<dyn Capture>> {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};
    let stop = Arc::new(AtomicBool::new(false));
    let (started, outcome) = mpsc::channel();
    let flag = stop.clone();
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let tell = move |result: Result<()>| {
                let _ = started.send(result);
            };
            if let Err(error) = capture(&flag, &tell) {
                tell(Err(error.clone()));
                if !flag.load(std::sync::atomic::Ordering::SeqCst) {
                    failed(error);
                }
            }
        })
        .map_err(|error| SpeechError::io(&error, "system audio thread"))?;
    match outcome.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(Box::new(Running { stop })),
        Ok(Err(error)) => Err(error),
        Err(_) => {
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
            Err(SpeechError::failed(
                "system_audio_unavailable",
                "System audio didn't start. Try again.",
            ))
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) use macos::{microphone_in_use, open};

/// A Core Audio process tap of every app's output, mixed to mono, in a
/// private aggregate device, adapted from anarlog's
/// `crates/audio-actual/src/speaker/macos.rs`. The real-time callback only
/// copies samples into a lock-free ring; a worker thread resamples them.
#[cfg(target_os = "macos")]
mod macos {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use cidre::core_audio as ca;
    use cidre::{cat, cf, ns, os};

    use super::*;
    use crate::audio::MonoResampler;

    const RING: usize = 1 << 18;

    struct Ctx {
        producer: rtrb::Producer<f32>,
        float: bool,
    }

    extern "C" fn proc(
        _device: ca::Device,
        _now: &cat::AudioTimeStamp,
        input: &cat::AudioBufList<1>,
        _input_time: &cat::AudioTimeStamp,
        _output: &mut cat::AudioBufList<1>,
        _output_time: &cat::AudioTimeStamp,
        ctx: Option<&mut Ctx>,
    ) -> os::Status {
        let Some(ctx) = ctx else {
            return os::Status::NO_ERR;
        };
        let buffer = &input.buffers[0];
        if !ctx.float || buffer.data.is_null() || buffer.data_bytes_size == 0 {
            return os::Status::NO_ERR;
        }
        let data = buffer.data as *const f32;
        if !(data as usize).is_multiple_of(std::mem::align_of::<f32>()) {
            return os::Status::NO_ERR;
        }
        let count = buffer.data_bytes_size as usize / std::mem::size_of::<f32>();
        // SAFETY: Core Audio hands a buffer of `data_bytes_size` bytes of
        // the tap's format, which `float` says is f32, and keeps it alive
        // for the call.
        let samples = unsafe { std::slice::from_raw_parts(data, count) };
        for sample in samples {
            if ctx.producer.push(*sample).is_err() {
                break;
            }
        }
        os::Status::NO_ERR
    }

    fn unavailable() -> SpeechError {
        SpeechError::failed(
            "system_audio_unavailable",
            "Noura can't record system audio. It needs macOS 14.4 or later and the \
             permission to record system audio.",
        )
    }

    pub(in crate::audio) fn open(sink: AudioSink, failed: FailureSink) -> Result<Box<dyn Capture>> {
        spawn(
            "noura-system-audio",
            move |stop, started| run(stop, started, &sink),
            failed,
        )
    }

    fn run(stop: &AtomicBool, started: &dyn Fn(Result<()>), sink: &AudioSink) -> Result<()> {
        let tap_desc = ca::TapDesc::with_mono_global_tap_excluding_processes(&ns::Array::new());
        let tap = tap_desc.create_process_tap().map_err(|_| unavailable())?;
        let tap_uid = tap.uid().map_err(|_| unavailable())?;
        let asbd = tap.asbd().map_err(|_| unavailable())?;
        let sub_tap = cf::DictionaryOf::with_keys_values(
            &[ca::sub_device_keys::uid()],
            &[tap_uid.as_type_ref()],
        );
        let agg_desc = cf::DictionaryOf::with_keys_values(
            &[
                ca::aggregate_device_keys::is_private(),
                ca::aggregate_device_keys::tap_auto_start(),
                ca::aggregate_device_keys::name(),
                ca::aggregate_device_keys::uid(),
                ca::aggregate_device_keys::tap_list(),
            ],
            &[
                cf::Boolean::value_true().as_type_ref(),
                cf::Boolean::value_false(),
                cf::String::from_str("noura-system-audio").as_ref(),
                &cf::Uuid::new().to_cf_string(),
                &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
            ],
        );
        let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(RING);
        let float = asbd.format_flags.contains(cat::AudioFormatFlags::IS_FLOAT)
            && asbd.bits_per_channel == 32;
        let mut ctx = Box::new(Ctx { producer, float });
        let device = ca::AggregateDevice::with_desc(&agg_desc).map_err(|_| unavailable())?;
        let proc_id = device
            .create_io_proc_id(proc, Some(&mut *ctx))
            .map_err(|_| unavailable())?;
        let started_device = ca::device_start(device, Some(proc_id)).map_err(|_| unavailable())?;
        started(Ok(()));
        let mut resampler = MonoResampler::new(asbd.sample_rate as u32);
        let mut scratch = Vec::with_capacity(4096);
        while !stop.load(Ordering::SeqCst) {
            scratch.clear();
            while let Ok(sample) = consumer.pop() {
                scratch.push(sample);
                if scratch.len() == 4096 {
                    break;
                }
            }
            if scratch.is_empty() {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            resampler.push(&scratch, &mut |samples| sink(samples));
        }
        drop(started_device);
        drop(ctx);
        drop(tap);
        Ok(())
    }

    /// Whether some app has the default microphone running, from Core
    /// Audio's "running somewhere" property.
    pub(in crate::audio) fn microphone_in_use() -> Option<bool> {
        let device = ca::System::default_input_device().ok()?;
        device
            .bool_prop(&ca::PropSelector::DEVICE_IS_RUNNING_SOMEWHERE.global_addr())
            .ok()
    }
}

#[cfg(target_os = "windows")]
pub(super) use windows::open;

/// A WASAPI loopback stream of the default output device, opened at 16 kHz
/// with WASAPI's own conversion and mixed to mono, adapted from anarlog's
/// `crates/audio-actual/src/speaker/windows.rs`. It moves to the new
/// default device when you switch outputs.
#[cfg(target_os = "windows")]
mod windows {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use wasapi::{
        AudioCaptureClient, AudioClient, Device, DeviceEnumerator, Direction, Handle, SampleType,
        StreamMode, WaveFormat, initialize_mta,
    };

    use super::*;
    use crate::engine::SAMPLE_RATE;

    const POLL: Duration = Duration::from_secs(1);

    struct Loopback {
        client: AudioClient,
        event: Handle,
        capture: AudioCaptureClient,
        channels: usize,
        device: String,
    }

    fn unavailable() -> SpeechError {
        SpeechError::failed(
            "system_audio_unavailable",
            "Noura can't record system audio from this output device.",
        )
    }

    fn open_loopback(device: Device) -> Result<Loopback> {
        let id = device.get_id().map_err(|_| unavailable())?;
        let mut client = device.get_iaudioclient().map_err(|_| unavailable())?;
        let mix = client.get_mixformat().map_err(|_| unavailable())?;
        let channels = mix.get_nchannels() as usize;
        let format = WaveFormat::new(
            32,
            32,
            SampleType::Float,
            SAMPLE_RATE as usize,
            channels,
            Some(mix.get_dwchannelmask()),
        );
        let (_, period) = client.get_device_period().map_err(|_| unavailable())?;
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: period,
        };
        client
            .initialize_client(&format, Direction::Capture, mode, None)
            .map_err(|_| unavailable())?;
        let event = client.set_get_eventhandle().map_err(|_| unavailable())?;
        let capture = client.get_audiocaptureclient().map_err(|_| unavailable())?;
        client.start_stream().map_err(|_| unavailable())?;
        Ok(Loopback {
            client,
            event,
            capture,
            channels: channels.max(1),
            device: id,
        })
    }

    pub(in crate::audio) fn open(sink: AudioSink, failed: FailureSink) -> Result<Box<dyn Capture>> {
        spawn(
            "noura-system-audio",
            move |stop, started| run(stop, started, &sink),
            failed,
        )
    }

    fn run(stop: &AtomicBool, started: &dyn Fn(Result<()>), sink: &AudioSink) -> Result<()> {
        initialize_mta().ok().map_err(|_| unavailable())?;
        let enumerator = DeviceEnumerator::new().map_err(|_| unavailable())?;
        let default = enumerator
            .get_default_device(Direction::Render)
            .map_err(|_| unavailable())?;
        let mut loopback = open_loopback(default)?;
        started(Ok(()));
        let mut queue = VecDeque::new();
        let mut mono = Vec::new();
        let mut next_poll = Instant::now() + POLL;
        while !stop.load(Ordering::SeqCst) {
            if Instant::now() >= next_poll {
                next_poll = Instant::now() + POLL;
                if let Ok(device) = enumerator.get_default_device(Direction::Render)
                    && device.get_id().is_ok_and(|id| id != loopback.device)
                    && let Ok(next) = open_loopback(device)
                {
                    let _ = loopback.client.stop_stream();
                    loopback = next;
                }
            }
            if loopback.event.wait_for_event(250).is_err() {
                continue;
            }
            queue.clear();
            while let Ok(Some(frames)) = loopback.capture.get_next_packet_size() {
                if frames == 0
                    || loopback
                        .capture
                        .read_from_device_to_deque(&mut queue)
                        .is_err()
                {
                    break;
                }
            }
            let bytes = queue.make_contiguous();
            let frame = loopback.channels * 4;
            mono.clear();
            for chunk in bytes.chunks_exact(frame) {
                let sum: f32 = chunk
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|sample| f32::from_le_bytes(*sample))
                    .sum();
                mono.push(sum / loopback.channels as f32);
            }
            if !mono.is_empty() {
                sink(&mono);
            }
        }
        let _ = loopback.client.stop_stream();
        Ok(())
    }
}
