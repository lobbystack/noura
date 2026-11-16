//! Mono resampling to 16 kHz with `rubato`, set up as `handy-recorder`
//! sets up its own resampler.

use rubato::audioadapter_buffers::direct::SequentialSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler, WindowFunction};

use crate::engine::SAMPLE_RATE;

const CHUNK: usize = 1024;

/// Turns mono audio at one rate into 16 kHz.
pub struct MonoResampler {
    resampler: Option<Fft<f32>>,
    input: Vec<f32>,
    output: Vec<f32>,
}

impl MonoResampler {
    /// A resampler from `rate` Hz. At 16 kHz it passes audio through.
    pub fn new(rate: u32) -> Self {
        let resampler = (rate != SAMPLE_RATE)
            .then(|| {
                Fft::<f32>::new_custom(
                    rate as usize,
                    SAMPLE_RATE as usize,
                    CHUNK,
                    1,
                    1,
                    WindowFunction::BlackmanHarris2,
                    FixedSync::Input,
                )
                .ok()
            })
            .flatten();
        let out = resampler.as_ref().map_or(0, |r| r.output_frames_max());
        Self {
            resampler,
            input: Vec::with_capacity(CHUNK),
            output: vec![0.0; out],
        }
    }

    /// Add `samples` and hand what's ready to `emit`.
    pub fn push(&mut self, mut samples: &[f32], emit: &mut dyn FnMut(&[f32])) {
        let Some(resampler) = self.resampler.as_mut() else {
            emit(samples);
            return;
        };
        while !samples.is_empty() {
            let take = (CHUNK - self.input.len()).min(samples.len());
            self.input.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.input.len() == CHUNK {
                let frames = self.output.len();
                let indexing = Indexing::new().partial_len(CHUNK);
                let produced = SequentialSlice::new(&self.input, 1, CHUNK)
                    .ok()
                    .zip(SequentialSlice::new_mut(&mut self.output, 1, frames).ok())
                    .and_then(|(input, mut output)| {
                        resampler
                            .process_into_buffer(&input, &mut output, Some(&indexing))
                            .ok()
                    })
                    .map_or(0, |(_, written)| written);
                self.input.clear();
                if produced > 0 {
                    emit(&self.output[..produced]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_second_at_48_khz_becomes_about_one_second_at_16_khz() {
        let mut resampler = MonoResampler::new(48_000);
        let mut out = 0;
        let tone: Vec<f32> = (0..48_000).map(|n| (n as f32 * 0.01).sin() * 0.2).collect();
        for piece in tone.chunks(441) {
            resampler.push(piece, &mut |samples| out += samples.len());
        }
        assert!((15_000..=16_000).contains(&out), "{out}");
        let mut same = MonoResampler::new(16_000);
        let mut passed = 0;
        same.push(&[0.0; 100], &mut |samples| passed += samples.len());
        assert_eq!(passed, 100);
    }
}
