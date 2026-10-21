//! Typing in a 1 MB note: replays trial S1's recorded keystrokes on its
//! 1 MB fixture and reports the 95th-percentile time from a keystroke's
//! arrival to the end of the frame that shows it, against the spec's
//! 16 ms budget.
//!
//! ```sh
//! cargo bench -p editor-egui --bench typing
//! ```
//!
//! CI reports the numbers on a macOS arm64 runner; each checkpoint holds
//! them to the budget on the owner's Mac. The replay renders on the GPU
//! when the machine has an adapter, and on the CPU otherwise.

use std::time::Duration;

#[allow(dead_code, reason = "the benchmark uses only part of the replay")]
#[path = "../tests/large_note/replay.rs"]
mod replay;
#[allow(dead_code, reason = "the benchmark uses only part of the script")]
#[path = "../tests/large_note/script.rs"]
mod script;

/// The spec's budget for the 95th-percentile keystroke-to-frame time.
const BUDGET: Duration = Duration::from_millis(16);

fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/large-note.md");
    let note = std::fs::read_to_string(path).expect("the 1 MB fixture is checked in");
    let mut replay = replay::Replay::open(&note, replay::Fonts::WithSystem, replay::RETINA, true);
    replay.settle(20);
    let samples = replay.run(&script::checked_in());
    let mut times: Vec<Duration> = samples.iter().map(|sample| sample.latency).collect();
    times.sort();

    let at = |percent: usize| times[(times.len() * percent).div_ceil(100).max(1) - 1];
    let ms = |time: Duration| format!("{:.1} ms", time.as_secs_f64() * 1000.0);
    let p95 = at(95);
    println!(
        "Typing in the 1 MB note, {} keystrokes, rendered {}",
        times.len(),
        if replay.renders_on_gpu() {
            "on the GPU"
        } else {
            "on the CPU, with no GPU adapter"
        }
    );
    println!(
        "  keystroke to frame: median {}, 95th percentile {}, slowest {}",
        ms(at(50)),
        ms(p95),
        ms(at(100))
    );
    println!(
        "  95th percentile against the {} budget: {}",
        ms(BUDGET),
        if p95 < BUDGET { "under" } else { "over" }
    );
}
