//! Trial S1's 1 MB note: the fixture, its keystroke script, and the
//! release-build replay that measures typing in it.
//!
//! The checks here run with `cargo test`. The measurements are ignored
//! tests that print a report. Run them one at a time, on an otherwise idle
//! machine:
//!
//! ```sh
//! cargo test -p editor-egui --release --test large_note -- \
//!     --ignored --nocapture --test-threads=1 measure_ replay_
//! ```

mod fixture;
mod replay;
mod script;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use editor_core::MdDoc;
use editor_core::comrak::nodes::{AstNode, NodeValue};
use editor_core::comrak::{Arena, parse_document};

fn checked_in_note() -> String {
    let bytes = std::fs::read(fixture::PATH).expect("the fixture is checked in");
    String::from_utf8(bytes).expect("the fixture is UTF-8")
}

#[test]
fn the_large_note_is_the_generators_output() {
    // A mismatch means the generator changed: rerun `write_the_large_note`.
    assert!(checked_in_note() == fixture::generate());
}

/// Counts of the constructs a note holds.
fn constructs(text: &str) -> BTreeMap<&'static str, usize> {
    let arena = Arena::new();
    let root = parse_document(&arena, text, &MdDoc::comrak_options());
    let mut counts = BTreeMap::new();
    let mut count = |name| *counts.entry(name).or_insert(0) += 1;
    for node in root.descendants() {
        match &node.data.borrow().value {
            NodeValue::FrontMatter(_) => count("frontmatter"),
            NodeValue::Heading(heading) => count(match heading.level {
                1 => "heading 1",
                2 => "heading 2",
                _ => "heading 3+",
            }),
            NodeValue::List(_) if inside_list(node) => count("nested list"),
            NodeValue::List(list) => count(match list.list_type {
                editor_core::comrak::nodes::ListType::Bullet => "bullet list",
                editor_core::comrak::nodes::ListType::Ordered => "ordered list",
            }),
            NodeValue::TaskItem(task) if task.symbol.is_some() => count("checked task"),
            NodeValue::TaskItem(_) => count("open task"),
            NodeValue::Table(_) => count("table"),
            NodeValue::CodeBlock(block) if !block.info.is_empty() => count("fenced code"),
            NodeValue::Link(link) if link.url.starts_with("https://") => count("link"),
            NodeValue::WikiLink(_) => count("wikilink"),
            NodeValue::BlockQuote | NodeValue::Alert(_) => count("quote or callout"),
            NodeValue::Paragraph if source_len(node) >= 1000 => count("long paragraph"),
            _ => {}
        }
    }
    counts
}

fn inside_list<'a>(node: &'a AstNode<'a>) -> bool {
    node.ancestors()
        .skip(1)
        .any(|ancestor| matches!(ancestor.data.borrow().value, NodeValue::Item(_)))
}

/// The paragraph's length in source bytes, counting one per column of its
/// single line.
fn source_len<'a>(node: &'a AstNode<'a>) -> usize {
    let pos = node.data.borrow().sourcepos;
    if pos.start.line == pos.end.line {
        pos.end.column.saturating_sub(pos.start.column)
    } else {
        0
    }
}

#[test]
fn the_large_note_holds_what_real_notes_hold() {
    let note = checked_in_note();
    assert!(
        (1_000_000..1_100_000).contains(&note.len()),
        "the note is {} bytes",
        note.len()
    );

    let counts = constructs(&note);
    for (construct, at_least) in [
        ("frontmatter", 1),
        ("heading 1", 10),
        ("heading 2", 500),
        ("heading 3+", 300),
        ("bullet list", 500),
        ("ordered list", 100),
        ("nested list", 500),
        ("open task", 500),
        ("checked task", 200),
        ("table", 100),
        ("fenced code", 100),
        ("link", 1000),
        ("wikilink", 1000),
        ("quote or callout", 100),
        ("long paragraph", 100),
    ] {
        let found = counts.get(construct).copied().unwrap_or(0);
        assert!(
            found >= at_least,
            "{found} of {construct}, expected {at_least}+"
        );
    }
}

#[test]
fn the_keystroke_script_covers_the_trial_and_finds_its_anchors() {
    use script::{Goto, Step};

    let note = checked_in_note();
    let steps = script::checked_in();
    let mut gotos = Vec::new();
    let mut kinds = std::collections::BTreeSet::new();
    for step in &steps {
        match step {
            Step::Goto(goto) => {
                if let Goto::Before(anchor) | Goto::After(anchor) = goto {
                    assert_eq!(note.matches(anchor.as_str()).count(), 1, "{anchor:?}");
                }
                gotos.push(goto.clone());
            }
            Step::Keys { kind, .. } => {
                kinds.insert(kind.as_str());
            }
        }
    }

    // Typing at the start of the body, in the middle and at the end.
    assert_eq!(
        gotos,
        [
            Goto::Before(fixture::FIRST_SENTENCE.into()),
            Goto::After(fixture::MIDDLE_SENTENCE.into()),
            Goto::End,
        ]
    );
    // Return, Backspace, a paste, and a selection that Backspace deletes.
    for kind in ["type", "Enter", "Backspace", "paste", "select"] {
        assert!(kinds.contains(kind), "the script has no {kind} step");
    }
    let select_then_delete = steps.windows(2).any(|pair| {
        matches!(
            pair,
            [Step::Keys { kind: select, .. }, Step::Keys { kind: delete, .. }]
                if select == "select" && delete == "Backspace"
        )
    });
    assert!(select_then_delete, "the script deletes a selection");
}

/// A short note holding the script's anchors, for replaying the script in
/// a debug build.
fn small_note() -> String {
    format!(
        "---\ntags: [team]\n---\n\n# Team notebook 2026\n\n{} It holds plans.\n\n\
         ## Midyear review\n\n{} More text here.\n\n\
         ## Year-end notes\n\n{} Done.\n",
        fixture::FIRST_SENTENCE,
        fixture::MIDDLE_SENTENCE,
        fixture::LAST_SENTENCE
    )
}

#[test]
fn the_keystroke_script_replays_on_a_small_note() {
    let mut replay = replay::Replay::open(&small_note(), replay::Fonts::Bundled, 1.0, false);
    replay.settle(5);
    let steps = script::checked_in();
    let samples = replay.run(&steps);

    assert_eq!(
        replay.editor.text(),
        "---\ntags: [team]\n---\n\n# Team notebook 2026\n\n\
         Updated weekly, newest entries last.\n\
         This notebook collects the team's working notes. It holds plans.\n\n\
         ## Midyear review\n\n\
         Halfway through the year, the team reviewed every open plan. \
         Most plans shipped on time, and two moved to the next quarter.\n\n\
         - Follow up with design\n\
         - Book the review room More text here.\n\n\
         ## Year-end notes\n\n\
         These are the last notes of the year. Done.\n\n\
         ## Wrap-up\n\n\
         The year closed with every open task checked."
    );
    let keystrokes: usize = steps
        .iter()
        .map(|step| match step {
            script::Step::Keys { events, .. } => events.len(),
            script::Step::Goto(_) => 0,
        })
        .sum();
    assert_eq!(samples.len(), keystrokes, "every keystroke was timed");
}

/// The value at or below which `percent` of `samples` fall (nearest rank).
fn percentile(samples: &[Duration], percent: usize) -> Duration {
    let mut sorted = samples.to_vec();
    sorted.sort();
    let rank = (sorted.len() * percent).div_ceil(100).max(1);
    sorted[rank - 1]
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

fn build() -> &'static str {
    if cfg!(debug_assertions) {
        "debug build: rerun with --release for the trial's numbers"
    } else {
        "release build"
    }
}

/// The median of `runs` timings of `work`.
fn median_of(runs: usize, mut work: impl FnMut()) -> Duration {
    let samples: Vec<Duration> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            work();
            start.elapsed()
        })
        .collect();
    percentile(&samples, 50)
}

/// Prints how long the 1 MB note takes to open and to parse.
#[test]
#[ignore = "measurement: run in a release build"]
fn measure_opening_and_parsing_the_large_note() {
    println!("Opening and parsing large-note.md ({})", build());

    let start = Instant::now();
    let bytes = std::fs::read(fixture::PATH).unwrap();
    let note = String::from_utf8(bytes).unwrap();
    let read = start.elapsed();
    println!("  bytes: {}", note.len());
    println!("  read the file and check UTF-8: {}", ms(read));

    // The parse the editor runs: comrak over the text plus a newline.
    let options = MdDoc::comrak_options();
    let with_newline = format!("{note}\n");
    let parse = median_of(5, || {
        let arena = Arena::new();
        parse_document(&arena, &with_newline, &options);
    });
    println!("  full comrak parse, median of 5: {}", ms(parse));

    // editor-core's reparse after a text change: the parse plus the
    // bounds it derives (source lines, words, folds, inline paragraphs).
    let reparse = median_of(5, || {
        let mut doc = MdDoc::new(&note);
        let arena = Arena::new();
        doc.reparse(&arena);
    });
    println!(
        "  editor-core reparse with bounds, median of 5: {}",
        ms(reparse)
    );

    let start = Instant::now();
    let mut replay = replay::Replay::open(&note, replay::Fonts::WithSystem, replay::RETINA, true);
    let created = start.elapsed();
    println!(
        "  rendering: {}",
        if replay.renders_on_gpu() {
            "GPU, offscreen through egui-wgpu"
        } else {
            "no wgpu adapter, CPU only"
        }
    );
    println!("  create the editor and load fonts: {}", ms(created));
    let start = Instant::now();
    let (first, _) = replay.frame(Vec::new());
    println!("  first frame: {}", ms(first));
    let frames = replay.settle(20);
    println!(
        "  from the editor's creation to a settled screen: {} ({} more frames)",
        ms(start.elapsed()),
        frames
    );

    let idle = median_of(10, || {
        replay.frame(Vec::new());
    });
    println!("  idle frame with no input, median of 10: {}", ms(idle));
}

/// Replays the keystroke script on the 1 MB note and prints the
/// keystroke-to-frame times against the spec's 16 ms budget.
#[test]
#[ignore = "measurement: run in a release build"]
fn replay_the_script_on_the_large_note() {
    println!("Replaying large-note.keys on large-note.md ({})", build());
    let note = checked_in_note();
    let mut replay = replay::Replay::open(&note, replay::Fonts::WithSystem, replay::RETINA, true);
    replay.settle(20);
    println!(
        "  rendering: {}",
        if replay.renders_on_gpu() {
            "GPU, offscreen through egui-wgpu"
        } else {
            "no wgpu adapter, CPU only"
        }
    );

    let steps = script::checked_in();
    let samples = replay.run(&steps);
    let all: Vec<Duration> = samples.iter().map(|sample| sample.latency).collect();
    let p95 = percentile(&all, 95);
    println!(
        "  {} keystrokes: median {}, 95th percentile {}, slowest {}",
        all.len(),
        ms(percentile(&all, 50)),
        ms(p95),
        ms(percentile(&all, 100))
    );
    println!(
        "  budget: 95th percentile under 16 ms: {}",
        if p95 < Duration::from_millis(16) {
            "pass"
        } else {
            "fail"
        }
    );

    let places = ["", "start", "middle", "end"];
    let mut groups: BTreeMap<(usize, String), Vec<Duration>> = BTreeMap::new();
    for sample in &samples {
        groups
            .entry((sample.place, sample.kind.clone()))
            .or_default()
            .push(sample.latency);
    }
    for ((place, kind), latencies) in groups {
        println!(
            "  {:<6} {:<9} n={:<3} median {:>9}  95th {:>9}  slowest {:>9}",
            places.get(place).copied().unwrap_or("?"),
            kind,
            latencies.len(),
            ms(percentile(&latencies, 50)),
            ms(percentile(&latencies, 95)),
            ms(percentile(&latencies, 100))
        );
    }

    // The replay typed where the script says.
    let text = replay.editor.text();
    for typed in [
        "Updated weekly, newest entries last.\nThis notebook collects",
        "every open plan. Most plans shipped on time",
        "- Follow up with design",
        "## Wrap-up\n\nThe year closed with every open task checked.",
    ] {
        assert!(text.contains(typed), "{typed:?} isn't in the note");
    }
}

/// Rewrites the checked-in note from the generator.
#[test]
#[ignore = "writes the fixture"]
fn write_the_large_note() {
    std::fs::write(fixture::PATH, fixture::generate()).unwrap();
}
