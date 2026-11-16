//! One recording: its captures, the threads that turn each channel's audio
//! into text, and what a plugin reads of it.
//!
//! Each channel runs on a thread of its own: it spools the audio to a WAV
//! file in the app data folder and either streams it to a model that
//! streams, or cuts it where you pause and queues each chunk. One worker
//! thread transcribes the queue in order, since a model computes one thing
//! at a time. Two channels never stream: both go through the queue.

use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use parking_lot::Mutex;

use crate::audio::{AudioBackend, Capture, Source};
use crate::chunker::{Chunk, Chunker};
use crate::engine::{Cancel, Live, Model, Options, Piece, SAMPLE_RATE, Transcriber, Vad};
use crate::{Result, SpeechError};

/// Where a recording is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Recording,
    Finishing,
    Finished,
    Failed,
    Cancelled,
}

impl State {
    pub fn ended(self) -> bool {
        matches!(self, State::Finished | State::Failed | State::Cancelled)
    }
}

/// A stretch of speech on one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub channel: Source,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// A recording as a plugin reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub id: String,
    pub state: State,
    pub elapsed_ms: u64,
    pub segments: Vec<Segment>,
    pub pending: Vec<Segment>,
    pub failure: Option<String>,
}

/// What changed in a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// It moved to another [`State`].
    State,
    /// New text arrived.
    Text,
}

pub type Changed = Arc<dyn Fn(&str, Change) + Send + Sync>;

/// How to record.
pub struct Plan {
    pub id: String,
    pub sources: Vec<Source>,
    pub microphone: Option<String>,
    pub options: Options,
    pub model: Arc<dyn Model>,
    /// A voice activity detector for each channel that doesn't stream.
    pub vad: Box<dyn Fn() -> Box<dyn Vad> + Send + Sync>,
    /// The folder the audio spools to.
    pub audio_folder: PathBuf,
}

struct Shared {
    snapshot: Mutex<Snapshot>,
    changed: Changed,
    /// Samples received per channel.
    received: Mutex<Vec<(Source, usize)>>,
}

impl Shared {
    fn set_state(&self, state: State, failure: Option<String>) {
        let id = {
            let mut snapshot = self.snapshot.lock();
            if snapshot.state.ended() || snapshot.state == state {
                return;
            }
            snapshot.state = state;
            if failure.is_some() {
                snapshot.failure = failure;
            }
            if state != State::Recording {
                snapshot.pending.clear();
            }
            snapshot.id.clone()
        };
        (self.changed)(&id, Change::State);
    }

    fn add(&self, pieces: Vec<Segment>) {
        if pieces.is_empty() {
            return;
        }
        let id = {
            let mut snapshot = self.snapshot.lock();
            if snapshot.state == State::Cancelled {
                return;
            }
            snapshot.segments.extend(pieces);
            snapshot
                .segments
                .sort_by_key(|segment| (segment.start_ms, segment.channel));
            snapshot.id.clone()
        };
        (self.changed)(&id, Change::Text);
    }

    fn live(&self, channel: Source, live: Live) {
        let id = {
            let mut snapshot = self.snapshot.lock();
            let elapsed = snapshot.elapsed_ms;
            snapshot.pending = vec![Segment {
                channel,
                start_ms: 0,
                end_ms: elapsed,
                text: format!("{}{}", live.committed, live.tentative)
                    .trim()
                    .to_owned(),
            }];
            snapshot.id.clone()
        };
        (self.changed)(&id, Change::Text);
    }

    fn heard(&self, channel: Source, samples: usize) {
        let elapsed = {
            let mut received = self.received.lock();
            match received.iter_mut().find(|(known, _)| *known == channel) {
                Some((_, count)) => *count += samples,
                None => received.push((channel, samples)),
            }
            received.iter().map(|(_, count)| *count).max().unwrap_or(0)
        };
        self.snapshot.lock().elapsed_ms = ms(elapsed);
    }
}

fn ms(samples: usize) -> u64 {
    samples as u64 * 1000 / u64::from(SAMPLE_RATE)
}

enum Job {
    Chunk(Source, Chunk),
    Done,
}

/// A recording in progress or ended.
pub struct Recording {
    pub id: String,
    shared: Arc<Shared>,
    captures: Mutex<Vec<Box<dyn Capture>>>,
    inputs: Mutex<Vec<Sender<Vec<f32>>>>,
    cancel: Cancel,
    audio_files: Vec<(Source, PathBuf)>,
}

/// The WAV file a channel's audio spools to.
fn spool(folder: &Path, id: &str, channel: Source) -> PathBuf {
    let name = match channel {
        Source::Microphone => "me",
        Source::SystemAudio => "others",
    };
    folder.join(format!("{id}-{name}.wav"))
}

fn wav_spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

impl Recording {
    /// Open the captures and start the threads. Fails, with nothing left
    /// running, when a device can't open.
    pub fn start(plan: Plan, audio: &dyn AudioBackend, changed: Changed) -> Result<Arc<Self>> {
        if plan.sources.is_empty() {
            return Err(SpeechError::failed(
                "no_source",
                "A recording needs at least one source.",
            ));
        }
        std::fs::create_dir_all(&plan.audio_folder)
            .map_err(|error| SpeechError::io(&error, "audio folder"))?;
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot {
                id: plan.id.clone(),
                state: State::Recording,
                elapsed_ms: 0,
                segments: Vec::new(),
                pending: Vec::new(),
                failure: None,
            }),
            changed,
            received: Mutex::new(Vec::new()),
        });
        let cancel = Cancel::new();
        let streaming = plan.model.streams() && plan.sources.len() == 1;
        let (jobs, queue) = mpsc::channel::<Job>();
        let mut inputs = Vec::new();
        let mut receivers = Vec::new();
        let mut audio_files = Vec::new();
        for channel in &plan.sources {
            let (input, received) = mpsc::channel::<Vec<f32>>();
            inputs.push(input);
            receivers.push((*channel, received));
            audio_files.push((*channel, spool(&plan.audio_folder, &plan.id, *channel)));
        }
        // Open every device before any thread starts, so a failure leaves
        // nothing behind.
        let mut captures = Vec::new();
        for (channel, input) in plan.sources.iter().zip(&inputs) {
            let sender = Mutex::new(input.clone());
            let failed_shared = shared.clone();
            let capture = audio.open(
                *channel,
                plan.microphone.as_deref(),
                Arc::new(move |samples: &[f32]| {
                    let _ = sender.lock().send(samples.to_vec());
                }),
                Arc::new(move |error: SpeechError| {
                    failed_shared.set_state(State::Failed, Some(error.message));
                }),
            )?;
            captures.push(capture);
        }
        let channels = plan.sources.len();
        for ((channel, received), (_, file)) in receivers.into_iter().zip(&audio_files) {
            let shared = shared.clone();
            let file = file.clone();
            let jobs = jobs.clone();
            let cancel = cancel.clone();
            let options = plan.options.clone();
            let model = plan.model.clone();
            let chunker = (!streaming).then(|| Chunker::new((plan.vad)()));
            thread::Builder::new()
                .name("noura-speech-channel".into())
                .spawn(move || {
                    channel_thread(
                        channel,
                        received,
                        &file,
                        chunker,
                        &jobs,
                        &shared,
                        streaming.then_some((model, options, cancel)),
                    );
                })
                .map_err(|error| SpeechError::io(&error, "channel thread"))?;
        }
        drop(jobs);
        if !streaming {
            let shared = shared.clone();
            let cancel = cancel.clone();
            let model = plan.model.clone();
            let options = plan.options.clone();
            thread::Builder::new()
                .name("noura-speech-worker".into())
                .spawn(move || worker(queue, channels, &*model, &options, &cancel, &shared))
                .map_err(|error| SpeechError::io(&error, "worker thread"))?;
        }
        Ok(Arc::new(Self {
            id: plan.id,
            shared,
            captures: Mutex::new(captures),
            inputs: Mutex::new(inputs),
            cancel,
            audio_files,
        }))
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot.lock().clone()
    }

    /// Stop recording; the threads transcribe what's left.
    pub fn stop(&self) {
        self.captures.lock().clear();
        self.inputs.lock().clear();
        self.shared.set_state(State::Finishing, None);
    }

    /// Stop recording and drop the audio and the text.
    pub fn cancel(&self) {
        self.cancel.cancel();
        self.captures.lock().clear();
        self.inputs.lock().clear();
        {
            let mut snapshot = self.shared.snapshot.lock();
            snapshot.segments.clear();
            snapshot.pending.clear();
        }
        self.shared.set_state(State::Cancelled, None);
        self.delete_audio();
    }

    /// The channels' spooled audio files.
    pub fn audio_files(&self) -> &[(Source, PathBuf)] {
        &self.audio_files
    }

    pub fn delete_audio(&self) {
        for (_, file) in &self.audio_files {
            let _ = std::fs::remove_file(file);
        }
    }

    /// The audio as one WAV file: the microphone on the left and the
    /// system audio on the right, or mono with one channel.
    pub fn wav_bytes(&self) -> Result<Vec<u8>> {
        let mut channels: Vec<Vec<i16>> = Vec::new();
        for (_, file) in &self.audio_files {
            let reader = hound::WavReader::open(file).map_err(|_| {
                SpeechError::failed("audio_missing", "This recording's audio is gone.")
            })?;
            channels.push(
                reader
                    .into_samples::<i16>()
                    .filter_map(|sample| sample.ok())
                    .collect(),
            );
        }
        let length = channels.iter().map(Vec::len).max().unwrap_or(0);
        let spec = hound::WavSpec {
            channels: channels.len().max(1) as u16,
            ..wav_spec()
        };
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut bytes, spec)
                .map_err(|_| SpeechError::failed("io", "Noura couldn't write the audio."))?;
            for index in 0..length {
                for channel in &channels {
                    writer
                        .write_sample(channel.get(index).copied().unwrap_or(0))
                        .map_err(|_| {
                            SpeechError::failed("io", "Noura couldn't write the audio.")
                        })?;
                }
            }
            writer
                .finalize()
                .map_err(|_| SpeechError::failed("io", "Noura couldn't write the audio."))?;
        }
        Ok(bytes.into_inner())
    }
}

fn channel_thread(
    channel: Source,
    received: Receiver<Vec<f32>>,
    file: &Path,
    mut chunker: Option<Chunker>,
    jobs: &Sender<Job>,
    shared: &Arc<Shared>,
    stream: Option<(Arc<dyn Model>, Options, Cancel)>,
) {
    let mut writer = std::fs::File::create(file)
        .ok()
        .and_then(|out| hound::WavWriter::new(BufWriter::new(out), wav_spec()).ok());
    if writer.is_none() {
        tracing::warn!("a channel's audio can't spool to disk");
    }
    // A streaming model gets the audio on its own thread.
    let (to_stream, streamer) = match stream {
        Some((model, options, cancel)) => {
            let (send, from_channel) = mpsc::channel::<Vec<f32>>();
            let shared = shared.clone();
            let handle = thread::spawn(move || {
                let mut session = match model.session() {
                    Ok(session) => session,
                    Err(error) => {
                        shared.set_state(State::Failed, Some(error.message));
                        return;
                    }
                };
                let live_shared = shared.clone();
                let result = session.stream(
                    &from_channel,
                    &options,
                    &mut |live| live_shared.live(channel, live),
                    &cancel,
                );
                match result {
                    Ok(pieces) => {
                        shared.add(segments(channel, 0, pieces));
                        shared.set_state(State::Finished, None);
                    }
                    Err(error) if error.code == "cancelled" => {}
                    Err(error) => shared.set_state(State::Failed, Some(error.message)),
                }
            });
            (Some(send), Some(handle))
        }
        None => (None, None),
    };
    while let Ok(samples) = received.recv() {
        shared.heard(channel, samples.len());
        if let Some(writer) = writer.as_mut() {
            for sample in &samples {
                let _ = writer.write_sample(to_i16(*sample));
            }
        }
        if let Some(chunker) = chunker.as_mut() {
            for chunk in chunker.push(&samples) {
                let _ = jobs.send(Job::Chunk(channel, chunk));
            }
        }
        if let Some(to_stream) = &to_stream {
            let _ = to_stream.send(samples);
        }
    }
    if let Some(writer) = writer {
        let _ = writer.finalize();
    }
    if let Some(chunker) = chunker.as_mut()
        && let Some(chunk) = chunker.finish()
    {
        let _ = jobs.send(Job::Chunk(channel, chunk));
    }
    let _ = jobs.send(Job::Done);
    drop(to_stream);
    if let Some(streamer) = streamer {
        let _ = streamer.join();
    }
}

fn segments(channel: Source, offset_samples: usize, pieces: Vec<Piece>) -> Vec<Segment> {
    let offset = ms(offset_samples);
    pieces
        .into_iter()
        .filter(|piece| !piece.text.trim().is_empty())
        .map(|piece| Segment {
            channel,
            start_ms: offset + piece.start_ms,
            end_ms: offset + piece.end_ms.max(piece.start_ms),
            text: piece.text.trim().to_owned(),
        })
        .collect()
}

fn worker(
    queue: Receiver<Job>,
    mut channels: usize,
    model: &dyn Model,
    options: &Options,
    cancel: &Cancel,
    shared: &Shared,
) {
    let mut session: Option<Box<dyn Transcriber>> = None;
    while channels > 0 {
        let Ok(job) = queue.recv() else {
            break;
        };
        let (channel, chunk) = match job {
            Job::Done => {
                channels -= 1;
                continue;
            }
            Job::Chunk(channel, chunk) => (channel, chunk),
        };
        if cancel.is_cancelled() {
            continue;
        }
        if session.is_none() {
            match model.session() {
                Ok(opened) => session = Some(opened),
                Err(error) => {
                    shared.set_state(State::Failed, Some(error.message));
                    return;
                }
            }
        }
        let Some(transcriber) = session.as_mut() else {
            return;
        };
        match transcriber.transcribe(&chunk.audio, options, cancel) {
            Ok(pieces) => {
                let mut found = segments(channel, chunk.start, pieces);
                // A model without timestamps gets the chunk's span.
                for segment in &mut found {
                    if segment.end_ms <= segment.start_ms {
                        segment.start_ms = ms(chunk.start);
                        segment.end_ms = ms(chunk.start + chunk.audio.len());
                    }
                }
                shared.add(found);
            }
            Err(error) if error.code == "cancelled" => return,
            Err(error) => {
                shared.set_state(State::Failed, Some(error.message));
                return;
            }
        }
    }
    if !cancel.is_cancelled() {
        shared.set_state(State::Finished, None);
    }
}
