//! Turning audio into text. The kernel talks to models through [`Engine`],
//! so tests run with [`FakeEngine`] and the app with transcribe.cpp, which
//! loads GGUF models on this device.
//!
//! All audio here is 16 kHz mono `f32` in `[-1, 1]`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use parking_lot::Mutex;

use crate::SpeechError;

/// Samples per second of every buffer the engine sees.
pub const SAMPLE_RATE: u32 = 16_000;

/// A stretch of text and when it was said, from the start of the audio
/// the engine got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// What to transcribe with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// An ISO 639-1 code, or `None` to let the model tell.
    pub language: Option<String>,
}

/// What runs when a [`Cancel`] fires.
type Hook = Box<dyn Fn() + Send + Sync>;

/// Stops a transcription. Clones share one flag; [`Cancel::cancel`] also
/// reaches the engine's own token, so a long run stops midway.
#[derive(Clone, Default)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
    hooks: Arc<Mutex<Vec<Hook>>>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        for hook in self.hooks.lock().iter() {
            hook();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Run `hook` when this is cancelled, at once if it already is.
    pub fn on_cancel(&self, hook: impl Fn() + Send + Sync + 'static) {
        if self.is_cancelled() {
            hook();
        }
        self.hooks.lock().push(Box::new(hook));
    }
}

/// What a stream says while you speak.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Live {
    /// Text the model won't change.
    pub committed: String,
    /// Text it may still change.
    pub tentative: String,
}

/// Loads models.
pub trait Engine: Send + Sync {
    /// Load the speech-to-text model in the GGUF file at `path`.
    fn load(&self, path: &Path) -> Result<Arc<dyn Model>, SpeechError>;

    /// Load a voice activity model, such as Silero's, from `path`.
    fn load_vad(&self, path: &Path) -> Result<Box<dyn Vad>, SpeechError>;
}

/// A loaded model. Several sessions may share it; they take turns.
pub trait Model: Send + Sync {
    /// Whether it shows text while you speak.
    fn streams(&self) -> bool;

    fn session(&self) -> Result<Box<dyn Transcriber>, SpeechError>;
}

/// One session of a model.
pub trait Transcriber: Send {
    /// Transcribe `pcm` in one go.
    fn transcribe(
        &mut self,
        pcm: &[f32],
        options: &Options,
        cancel: &Cancel,
    ) -> Result<Vec<Piece>, SpeechError>;

    /// Transcribe the audio `input` sends while it arrives, calling `live`
    /// as the text changes, until `input` closes. Returns the final text.
    /// Only for a model that [`Model::streams`].
    fn stream(
        &mut self,
        input: &Receiver<Vec<f32>>,
        options: &Options,
        live: &mut dyn FnMut(Live),
        cancel: &Cancel,
    ) -> Result<Vec<Piece>, SpeechError>;
}

/// Tells speech from silence, one frame at a time.
pub trait Vad: Send {
    /// Samples per frame.
    fn frame_samples(&self) -> usize;

    /// How likely the frame holds speech, from 0 to 1.
    fn speech(&mut self, frame: &[f32]) -> f32;
}

/// Tells speech by loudness alone, for when the voice activity model isn't
/// on this device, and for tests.
pub struct EnergyVad {
    /// The RMS level that counts as fully speech.
    pub loud: f32,
}

impl Default for EnergyVad {
    fn default() -> Self {
        Self { loud: 0.02 }
    }
}

impl Vad for EnergyVad {
    fn frame_samples(&self) -> usize {
        512
    }

    fn speech(&mut self, frame: &[f32]) -> f32 {
        if frame.is_empty() {
            return 0.0;
        }
        let rms =
            (frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len() as f32).sqrt();
        (rms / self.loud).clamp(0.0, 1.0)
    }
}

/// An engine for tests. Its models answer each buffer with one piece whose
/// text says how long it was, as `1.5 s`, or with the next of the texts
/// you give it.
#[derive(Default)]
pub struct FakeEngine {
    pub streaming: bool,
    /// Answers, in order; once they run out, each buffer's length.
    pub texts: Arc<Mutex<Vec<String>>>,
    /// The length of every buffer transcribed so far, in samples.
    pub heard: Arc<Mutex<Vec<usize>>>,
    /// Makes loading fail, as for a broken file.
    pub broken: bool,
    /// Makes every transcription fail, as for a model the engine can't run.
    pub failing: bool,
}

impl Engine for FakeEngine {
    fn load(&self, path: &Path) -> Result<Arc<dyn Model>, SpeechError> {
        if self.broken || !path.exists() {
            return Err(SpeechError::failed(
                "model_unreadable",
                "Noura couldn't load the model. Download it again in Settings.",
            ));
        }
        Ok(Arc::new(FakeModel {
            streaming: self.streaming,
            texts: self.texts.clone(),
            heard: self.heard.clone(),
            failing: self.failing,
        }))
    }

    fn load_vad(&self, _path: &Path) -> Result<Box<dyn Vad>, SpeechError> {
        Ok(Box::new(EnergyVad::default()))
    }
}

struct FakeModel {
    streaming: bool,
    texts: Arc<Mutex<Vec<String>>>,
    heard: Arc<Mutex<Vec<usize>>>,
    failing: bool,
}

impl Model for FakeModel {
    fn streams(&self) -> bool {
        self.streaming
    }

    fn session(&self) -> Result<Box<dyn Transcriber>, SpeechError> {
        Ok(Box::new(FakeModel {
            streaming: self.streaming,
            texts: self.texts.clone(),
            heard: self.heard.clone(),
            failing: self.failing,
        }))
    }
}

impl FakeModel {
    fn failure() -> SpeechError {
        SpeechError::failed(
            "engine_failed",
            "Noura couldn't transcribe this audio. Try another model in Settings.",
        )
    }

    fn answer(&self, samples: usize) -> String {
        self.heard.lock().push(samples);
        let mut texts = self.texts.lock();
        if texts.is_empty() {
            format!("{:.1} s", samples as f64 / f64::from(SAMPLE_RATE))
        } else {
            texts.remove(0)
        }
    }
}

fn ms(samples: usize) -> u64 {
    samples as u64 * 1000 / u64::from(SAMPLE_RATE)
}

impl Transcriber for FakeModel {
    fn transcribe(
        &mut self,
        pcm: &[f32],
        _options: &Options,
        cancel: &Cancel,
    ) -> Result<Vec<Piece>, SpeechError> {
        if cancel.is_cancelled() {
            return Err(SpeechError::cancelled());
        }
        if self.failing {
            return Err(Self::failure());
        }
        Ok(vec![Piece {
            start_ms: 0,
            end_ms: ms(pcm.len()),
            text: self.answer(pcm.len()),
        }])
    }

    fn stream(
        &mut self,
        input: &Receiver<Vec<f32>>,
        _options: &Options,
        live: &mut dyn FnMut(Live),
        cancel: &Cancel,
    ) -> Result<Vec<Piece>, SpeechError> {
        if self.failing {
            return Err(Self::failure());
        }
        let mut total = 0;
        while let Ok(buffer) = input.recv() {
            if cancel.is_cancelled() {
                return Err(SpeechError::cancelled());
            }
            total += buffer.len();
            live(Live {
                committed: String::new(),
                tentative: format!("{:.1} s", total as f64 / f64::from(SAMPLE_RATE)),
            });
        }
        Ok(vec![Piece {
            start_ms: 0,
            end_ms: ms(total),
            text: self.answer(total),
        }])
    }
}

/// transcribe.cpp, which runs GGUF models on this device's CPU or GPU.
#[cfg(feature = "transcribe")]
pub use transcribe::TranscribeCpp;

#[cfg(feature = "transcribe")]
mod transcribe {
    use super::*;
    use transcribe_cpp as tc;

    /// The transcribe.cpp engine.
    #[derive(Default)]
    pub struct TranscribeCpp;

    fn failure(error: &tc::Error, what: &str) -> SpeechError {
        tracing::warn!(%error, what, "transcribe.cpp failed");
        SpeechError::failed(
            "engine_failed",
            "Noura couldn't transcribe this audio. Try another model in Settings.",
        )
    }

    impl Engine for TranscribeCpp {
        fn load(&self, path: &Path) -> Result<Arc<dyn Model>, SpeechError> {
            let model = tc::Model::load(path).map_err(|error| {
                tracing::warn!(%error, "transcribe.cpp couldn't load a model");
                SpeechError::failed(
                    "model_unreadable",
                    "Noura couldn't load the model. Download it again in Settings.",
                )
            })?;
            let streams = model
                .capabilities()
                .is_ok_and(|capabilities| capabilities.supports_streaming);
            Ok(Arc::new(Loaded { model, streams }))
        }

        fn load_vad(&self, path: &Path) -> Result<Box<dyn Vad>, SpeechError> {
            let model = tc::Model::load(path).map_err(|error| failure(&error, "vad load"))?;
            let frame = model
                .vad_info()
                .map_err(|error| failure(&error, "vad info"))?
                .frame_samples
                .max(1) as usize;
            let session = model
                .vad_session()
                .map_err(|error| failure(&error, "vad session"))?;
            Ok(Box::new(Silero {
                session,
                frame,
                last: 0.0,
            }))
        }
    }

    struct Loaded {
        model: tc::Model,
        streams: bool,
    }

    impl Model for Loaded {
        fn streams(&self) -> bool {
            self.streams
        }

        fn session(&self) -> Result<Box<dyn Transcriber>, SpeechError> {
            let session = self
                .model
                .session()
                .map_err(|error| failure(&error, "session"))?;
            Ok(Box::new(Session(session)))
        }
    }

    struct Session(tc::Session);

    fn run_options(options: &Options) -> tc::RunOptions {
        tc::RunOptions {
            language: options.language.clone(),
            ..tc::RunOptions::default()
        }
    }

    fn pieces(transcript: &tc::Transcript) -> Vec<Piece> {
        let mut pieces: Vec<Piece> = transcript
            .segments
            .iter()
            .filter(|segment| !segment.text.trim().is_empty())
            .map(|segment| Piece {
                start_ms: segment.t0_ms.max(0) as u64,
                end_ms: segment.t1_ms.max(segment.t0_ms).max(0) as u64,
                text: segment.text.trim().to_owned(),
            })
            .collect();
        if pieces.is_empty() && !transcript.text.trim().is_empty() {
            pieces.push(Piece {
                start_ms: 0,
                end_ms: 0,
                text: transcript.text.trim().to_owned(),
            });
        }
        pieces
    }

    fn token(cancel: &Cancel) -> tc::CancelToken {
        let token = tc::CancelToken::new();
        let shared = token.clone();
        cancel.on_cancel(move || shared.cancel());
        token
    }

    impl Transcriber for Session {
        fn transcribe(
            &mut self,
            pcm: &[f32],
            options: &Options,
            cancel: &Cancel,
        ) -> Result<Vec<Piece>, SpeechError> {
            self.0.set_cancel_token(&token(cancel));
            let result = self.0.run(pcm, &run_options(options));
            self.0.clear_cancel_token();
            if cancel.is_cancelled() {
                return Err(SpeechError::cancelled());
            }
            result
                .map(|transcript| pieces(&transcript))
                .map_err(|error| failure(&error, "run"))
        }

        fn stream(
            &mut self,
            input: &Receiver<Vec<f32>>,
            options: &Options,
            live: &mut dyn FnMut(Live),
            cancel: &Cancel,
        ) -> Result<Vec<Piece>, SpeechError> {
            self.0.set_cancel_token(&token(cancel));
            let result = (|| {
                let mut stream = self
                    .0
                    .stream(&run_options(options), &tc::StreamOptions::default())
                    .map_err(|error| failure(&error, "stream"))?;
                while let Ok(buffer) = input.recv() {
                    if cancel.is_cancelled() {
                        return Err(SpeechError::cancelled());
                    }
                    let update = stream
                        .feed(&buffer)
                        .map_err(|error| failure(&error, "feed"))?;
                    if update.result_changed {
                        let text = stream.text();
                        live(Live {
                            committed: text.committed,
                            tentative: text.tentative,
                        });
                    }
                }
                stream
                    .finalize()
                    .map_err(|error| failure(&error, "finalize"))?;
                Ok(pieces(&stream.snapshot()))
            })();
            self.0.clear_cancel_token();
            if cancel.is_cancelled() {
                return Err(SpeechError::cancelled());
            }
            result
        }
    }

    struct Silero {
        session: tc::VadSession,
        frame: usize,
        last: f32,
    }

    impl Vad for Silero {
        fn frame_samples(&self) -> usize {
            self.frame
        }

        fn speech(&mut self, frame: &[f32]) -> f32 {
            match self.session.stream_feed(frame) {
                Ok(result) => {
                    if let Some(last) = result.probs.last() {
                        self.last = *last;
                    }
                    self.last
                }
                Err(error) => {
                    tracing::warn!(%error, "Silero VAD failed on a frame");
                    self.last
                }
            }
        }
    }
}
