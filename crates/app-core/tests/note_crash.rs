//! Seam 1 with a process killed: Noura stops mid-typing, at any moment,
//! with no chance to save. The file then holds every save that returned,
//! whole, and the next launch opens that text. What the save window of 300
//! ms to 2 seconds hadn't reached the disk yet is lost.
//!
//! The test runs a copy of this test binary as the process it kills: the
//! ignored test `types_until_killed` types and saves on the same app data.

mod support;

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use app_core::{App, OpenAs, SAVE_PAUSE};
use support::{FakeSystem, Fixture, settle, settle_saves};

const APP_DATA: &str = "NOURA_TEST_KILLED_APP_DATA";
const NOTE: &str = "Plan.md";

/// The line of the note that typing number `index` adds.
fn line(index: usize) -> String {
    format!("Typed line {index} of the long note.\n")
}

/// The process the test kills: it opens the note and types for as long as
/// it runs, printing the number of each line whose save returned. Each
/// new line starts its save at once, so a kill often lands in a write.
#[test]
#[ignore = "the crash test runs it as a process of its own"]
fn types_until_killed() {
    let Some(app_data) = std::env::var_os(APP_DATA) else {
        return;
    };
    let mut app = App::launch_with(Path::new(&app_data), FakeSystem::new());
    settle(&mut app);
    app.open_file(NOTE, OpenAs::Kept);
    let tab = app.active_tab().expect("the note opens").id();
    let mut text = app.note(tab).expect("a note").text().to_owned();
    let mut index = 0;
    loop {
        text.push_str(&line(index));
        let at = Instant::now();
        app.edit_note(tab, 1, &text, at);
        if index % 2 == 0 {
            // Typing that a save has yet to reach when the kill lands.
            app.save_due_notes(at + SAVE_PAUSE);
            settle_saves(&mut app);
            println!("saved {index}");
        }
        index += 1;
    }
}

/// The body of `bytes` after the frontmatter.
fn body(bytes: &[u8]) -> String {
    match local_core::parse_markdown(NOTE, bytes) {
        local_core::ParsedMarkdown::Managed(object) => object.body,
        other => panic!("the file doesn't read as a note: {other:?}"),
    }
}

#[test]
fn a_kill_mid_typing_leaves_every_returned_save_whole_on_disk() {
    let bytes = support::note("01jbcdefghjkmnpqrstvwxyz0a", "");
    let fixture = Fixture::new(&[(NOTE, &bytes)]);
    let start = body(&bytes);

    // Each round kills a new process at a different moment in its saves.
    for round in 0..4 {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "--nocapture", "types_until_killed"])
            .env(APP_DATA, fixture.app_data.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // The reader stays open until the kill, so the process keeps
        // printing.
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let wanted = 3 + round * 7;
        let mut returned = None;
        let mut printed = String::new();
        while returned.is_none_or(|index| index < wanted) {
            printed.clear();
            assert!(
                output.read_line(&mut printed).unwrap() > 0,
                "the process stopped by itself"
            );
            if let Some(index) = printed.trim_end().strip_prefix("saved ") {
                returned = Some(index.parse::<usize>().unwrap());
            }
        }
        std::thread::sleep(Duration::from_millis(u64::try_from(round).unwrap() * 3));
        child.kill().unwrap();
        child.wait().unwrap();
        let returned = returned.expect("the process saved something");

        // The file holds whole lines: everything a save returned for and
        // perhaps more, never part of a write.
        let text = body(&fixture.read(NOTE));
        let typed = text
            .strip_prefix(&start)
            .unwrap_or_else(|| panic!("the file lost its start: {text:?}"));
        assert!(typed.ends_with('\n'), "a torn write: {typed:?}");
        assert!(
            typed.contains(&line(returned)),
            "the save of line {returned} is gone from {typed:?}"
        );
        drop(output);

        // Noura reopens the same text. A write the kill cut short can leave
        // its hidden temporary file beside the note, which the tree skips.
        let mut app = fixture.app();
        app.open_file(NOTE, OpenAs::Kept);
        let tab = app.active_tab().expect("the note opens").id();
        assert_eq!(app.note(tab).unwrap().text(), text);
        assert!(!app.note(tab).unwrap().is_unsaved());
        assert_eq!(support::rows(&app), ["Plan"]);
        drop(app);
        // The next process types on from what's on disk, so its numbers
        // start again and the last one it saved is the one to find.
    }
}
