//! Cutting a channel's audio where you pause, for models that can't
//! stream. Each chunk holds one stretch of speech with a little silence
//! around it, so the model transcribes it while you go on, and the text
//! of a meeting arrives a few seconds behind.

use crate::engine::{SAMPLE_RATE, Vad};

/// A frame at or above this is speech.
const SPEECH: f32 = 0.5;
/// Inside speech, a frame below this is silence.
const SILENCE: f32 = 0.35;
/// A pause this long ends a chunk.
const PAUSE_MS: u64 = 600;
/// Speech shorter than this, as a cough, is dropped.
const MIN_SPEECH_MS: u64 = 250;
/// Silence kept before and after the speech.
const PAD_MS: u64 = 200;
/// A chunk never grows past this, so a long monologue still arrives.
const MAX_CHUNK_MS: u64 = 25_000;

fn samples(ms: u64) -> usize {
    (ms * u64::from(SAMPLE_RATE) / 1000) as usize
}

/// A stretch of speech, and where it starts in the channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// The sample of the channel the chunk starts at.
    pub start: usize,
    pub audio: Vec<f32>,
}

/// Feeds audio through a [`Vad`] and hands out the chunks it finds.
pub struct Chunker {
    vad: Box<dyn Vad>,
    frame: usize,
    /// Audio not yet scored, less than a frame.
    pending: Vec<f32>,
    /// The audio since `kept_from`: the padding before speech, then the
    /// chunk so far.
    kept: Vec<f32>,
    /// The channel sample `kept[0]` is.
    kept_from: usize,
    /// Where the speech of the chunk in progress starts in `kept`.
    speech_at: Option<usize>,
    /// Samples of speech in the chunk in progress.
    speech_samples: usize,
    /// Samples of silence since the last speech frame.
    quiet: usize,
    in_speech: bool,
}

impl Chunker {
    pub fn new(vad: Box<dyn Vad>) -> Self {
        let frame = vad.frame_samples().max(1);
        Self {
            vad,
            frame,
            pending: Vec::new(),
            kept: Vec::new(),
            kept_from: 0,
            speech_at: None,
            speech_samples: 0,
            quiet: 0,
            in_speech: false,
        }
    }

    /// Add audio, and return the chunks it completes.
    pub fn push(&mut self, audio: &[f32]) -> Vec<Chunk> {
        let mut done = Vec::new();
        self.pending.extend_from_slice(audio);
        let whole = self.pending.len() / self.frame * self.frame;
        let frames: Vec<f32> = self.pending.drain(..whole).collect();
        for frame in frames.chunks(self.frame) {
            if let Some(chunk) = self.frame(frame) {
                done.push(chunk);
            }
        }
        done
    }

    /// The chunk in progress, once the audio ends.
    pub fn finish(&mut self) -> Option<Chunk> {
        let pending = std::mem::take(&mut self.pending);
        self.kept.extend_from_slice(&pending);
        self.cut(self.kept.len())
    }

    fn frame(&mut self, frame: &[f32]) -> Option<Chunk> {
        let probability = self.vad.speech(frame);
        self.in_speech = if self.in_speech {
            probability >= SILENCE
        } else {
            probability >= SPEECH
        };
        let at = self.kept.len();
        self.kept.extend_from_slice(frame);
        if self.in_speech {
            if self.speech_at.is_none() {
                self.speech_at = Some(at);
            }
            self.speech_samples += frame.len();
            self.quiet = 0;
        } else if self.speech_at.is_some() {
            self.quiet += frame.len();
        }
        match self.speech_at {
            None => {
                // Keep only the padding before speech starts.
                let pad = samples(PAD_MS);
                if self.kept.len() > pad {
                    let drop = self.kept.len() - pad;
                    self.kept.drain(..drop);
                    self.kept_from += drop;
                }
                None
            }
            Some(_) if self.quiet >= samples(PAUSE_MS) => {
                // End the chunk partway through the pause.
                let end = self.kept.len() - self.quiet + samples(PAD_MS).min(self.quiet);
                self.cut(end)
            }
            Some(start) if self.kept.len() - start >= samples(MAX_CHUNK_MS) => {
                let end = self.kept.len();
                self.cut(end)
            }
            Some(_) => None,
        }
    }

    /// Hand out `kept[..end]` as a chunk, when it holds enough speech, and
    /// start again after it.
    fn cut(&mut self, end: usize) -> Option<Chunk> {
        let had_speech = self.speech_at.is_some() && self.speech_samples >= samples(MIN_SPEECH_MS);
        let audio: Vec<f32> = self.kept.drain(..end.min(self.kept.len())).collect();
        let start = self.kept_from;
        self.kept_from += audio.len();
        self.speech_at = None;
        self.speech_samples = 0;
        self.quiet = 0;
        self.in_speech = false;
        (had_speech && !audio.is_empty()).then_some(Chunk { start, audio })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::EnergyVad;

    /// `ms` of a loud tone, then `ms` of silence, as a speaker who pauses.
    fn speech_then_silence(talk_ms: u64, quiet_ms: u64) -> Vec<f32> {
        let mut audio: Vec<f32> = (0..samples(talk_ms))
            .map(|n| 0.3 * (n as f32 * 0.07).sin())
            .collect();
        audio.extend(std::iter::repeat_n(0.0, samples(quiet_ms)));
        audio
    }

    fn chunks(audio: &[f32]) -> Vec<Chunk> {
        let mut chunker = Chunker::new(Box::new(EnergyVad::default()));
        let mut found = Vec::new();
        for piece in audio.chunks(480) {
            found.extend(chunker.push(piece));
        }
        found.extend(chunker.finish());
        found
    }

    #[test]
    fn each_stretch_of_speech_becomes_a_chunk_at_its_place() {
        let mut audio = speech_then_silence(1500, 1000);
        audio.extend(speech_then_silence(2000, 1000));
        let found = chunks(&audio);
        assert_eq!(
            found.len(),
            2,
            "{:?}",
            found
                .iter()
                .map(|c| (c.start, c.audio.len()))
                .collect::<Vec<_>>()
        );
        // The first starts at the start; the second just before its speech.
        assert_eq!(found[0].start, 0);
        let second_speech = samples(2500);
        assert!(
            found[1].start <= second_speech
                && found[1].start + samples(PAD_MS) + 1024 >= second_speech
        );
        // Each holds its speech and some padding, not the whole pause.
        assert!(found[0].audio.len() >= samples(1500));
        assert!(found[0].audio.len() < samples(2400));
    }

    #[test]
    fn silence_and_a_cough_make_no_chunk() {
        assert!(chunks(&vec![0.0; samples(5000)]).is_empty());
        assert!(chunks(&speech_then_silence(100, 2000)).is_empty());
    }

    #[test]
    fn a_long_monologue_arrives_in_pieces() {
        let found = chunks(&speech_then_silence(60_000, 100));
        assert!(found.len() >= 3, "{}", found.len());
        assert!(
            found
                .iter()
                .all(|chunk| chunk.audio.len() <= samples(MAX_CHUNK_MS) + 512)
        );
        let total: usize = found.iter().map(|chunk| chunk.audio.len()).sum();
        assert!(total >= samples(60_000));
    }
}
