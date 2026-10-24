//! Links in the open note, through the window: a click on a drawn link
//! opens its target under the preview-tab rules, a click on visible link
//! source places the caret, Cmd-click, Ctrl-click and a middle-click open
//! a new tab, Alt-Enter follows the link at the caret, and typing `[[`
//! lists the workspace's files. Tab state comes from `app-core`.

mod support;

use std::path::Path;

use app_core::Platform;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable},
};
use support::{create_workspace, harness_for, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const PLAN: &str = "See [[Other]] and [the draft](Draft.md).\n";

/// A workspace whose note `Plan` links to `Other` and `Draft`, with
/// `Plan` opened from the tree in a preview tab.
fn opened_plan(app_data: &Path, folder: &Path, platform: Platform) -> Ui {
    create_workspace(app_data, "Garden", folder);
    std::fs::write(folder.join("Plan.md"), PLAN).unwrap();
    std::fs::write(folder.join("Other.md"), "Other\n").unwrap();
    std::fs::write(folder.join("Draft.md"), "Draft\n").unwrap();
    let mut harness = harness_for(app_data, platform, |_| {});
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    assert_eq!(tabs(&harness), [("Plan".to_owned(), true)]);
    harness
}

/// Each tab's title and whether it's the preview, as `app-core` has them.
fn tabs(harness: &Ui) -> Vec<(String, bool)> {
    harness
        .state()
        .app()
        .tabs()
        .iter()
        .map(|tab| (tab.title().to_owned(), tab.is_preview()))
        .collect()
}

fn active(harness: &Ui) -> Option<String> {
    let app = harness.state().app();
    app.active_tab().map(|tab| tab.title().to_owned())
}

/// The open note's text as the editor last reported it.
fn note_text(harness: &Ui) -> String {
    let app = harness.state().app();
    let tab = app.active_tab().expect("a tab is open").id();
    app.note(tab)
        .expect("the tab shows a note")
        .text()
        .to_owned()
}

/// Where the `nth` character of the first `needle` in the note drew, from
/// the text runs the editor reports to assistive technology.
fn point_at(harness: &Ui, needle: &str, nth: usize) -> Pos2 {
    let note = harness.get_by_role_and_label(Role::MultilineTextInput, "Note text");
    let text = note.value().expect("the note reports its text");
    let target = text.find(needle).expect("the note holds the needle")
        + needle
            .char_indices()
            .nth(nth)
            .expect("the needle is long enough")
            .0;
    let mut run_start = 0;
    for run in note.children() {
        let run = run.accesskit_node();
        let data = run.data();
        if data.role() != Role::TextRun {
            continue;
        }
        let value = data.value().unwrap_or_default();
        let mut at = run_start;
        for (index, length) in data.character_lengths().iter().enumerate() {
            if target < at + usize::from(*length) {
                let bounds = data.bounds().expect("the run drew");
                let left = data.character_positions().expect("the run drew")[index];
                let width = data.character_widths().expect("the run drew")[index];
                assert!(width > 0.0, "{needle:?} drew nothing at {nth}");
                return Pos2::new(
                    bounds.x0 as f32 + left + width / 2.0,
                    ((bounds.y0 + bounds.y1) / 2.0) as f32,
                );
            }
            at += usize::from(*length);
        }
        run_start += value.len();
    }
    panic!("no text run holds {needle:?}");
}

/// Click `button` at `pos` with `modifiers` held, as a person does: the
/// pointer and the keys get there a few frames before the click.
fn click_at(harness: &mut Ui, pos: Pos2, button: PointerButton, modifiers: Modifiers) {
    harness.event(Event::PointerMoved(pos));
    harness.event(Event::ModifiersChanged(modifiers));
    harness.run_steps(3);
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        });
    }
    harness.event(Event::ModifiersChanged(Modifiers::NONE));
    harness.run_steps(6);
}

fn press(harness: &mut Ui, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run_steps(3);
}

/// Move the caret into the Markdown link's source from the keyboard, so
/// the source shows: to the end of the line, then back past `.` and `)`.
fn caret_in_draft_link(harness: &mut Ui) {
    press(harness, Modifiers::NONE, Key::End);
    press(harness, Modifiers::NONE, Key::ArrowLeft);
    press(harness, Modifiers::NONE, Key::ArrowLeft);
}

/// The caret's offset in the note's characters, as the editor reports it
/// to assistive technology.
fn caret(harness: &Ui) -> usize {
    let note = harness.get_by_role_and_label(Role::MultilineTextInput, "Note text");
    let focus = note
        .accesskit_node()
        .data()
        .text_selection()
        .expect("the note reports its selection")
        .focus;
    let mut before = 0;
    for run in note.children() {
        let run = run.accesskit_node();
        if run.locate().0 == focus.node {
            return before + focus.character_index;
        }
        before += run.data().character_lengths().len();
    }
    panic!("no text run holds the caret");
}

#[test]
fn a_click_on_a_drawn_link_opens_its_target_in_the_preview_tab() {
    for (needle, target) in [("Other", "Other"), ("the draft", "Draft")] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let mut harness = opened_plan(
            app_data.path(),
            &parent.path().join("Garden"),
            Platform::Windows,
        );

        let pos = point_at(&harness, needle, 1);
        click_at(&mut harness, pos, PointerButton::Primary, Modifiers::NONE);

        assert_eq!(tabs(&harness), [(target.to_owned(), true)], "{needle:?}");
    }
}

#[test]
fn a_click_on_visible_link_source_places_the_caret() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened_plan(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::Windows,
    );
    caret_in_draft_link(&mut harness);

    let pos = point_at(&harness, "Draft.md", 2);
    click_at(&mut harness, pos, PointerButton::Primary, Modifiers::NONE);

    assert_eq!(tabs(&harness), [("Plan".to_owned(), true)]);
    let destination = PLAN.find("Draft.md").unwrap();
    assert!(
        (destination + 2..=destination + 3).contains(&caret(&harness)),
        "the caret went to {}",
        caret(&harness)
    );
}

#[test]
fn cmd_click_ctrl_click_and_middle_click_open_a_new_tab() {
    let ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    let cmd = Modifiers {
        mac_cmd: true,
        command: true,
        ..Modifiers::NONE
    };
    for (platform, source, button, modifiers) in [
        (Platform::Windows, false, PointerButton::Primary, ctrl),
        (Platform::MacOs, false, PointerButton::Primary, cmd),
        (
            Platform::Windows,
            false,
            PointerButton::Middle,
            Modifiers::NONE,
        ),
        (Platform::Windows, true, PointerButton::Primary, ctrl),
        (
            Platform::Windows,
            true,
            PointerButton::Middle,
            Modifiers::NONE,
        ),
    ] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let mut harness = opened_plan(app_data.path(), &parent.path().join("Garden"), platform);
        let pos = if source {
            caret_in_draft_link(&mut harness);
            point_at(&harness, "Draft.md", 2)
        } else {
            point_at(&harness, "the draft", 1)
        };

        click_at(&mut harness, pos, button, modifiers);

        let case = format!("{platform:?}, on source: {source}, {button:?}");
        assert_eq!(
            tabs(&harness),
            [("Plan".to_owned(), true), ("Draft".to_owned(), false)],
            "{case}"
        );
        assert_eq!(active(&harness).as_deref(), Some("Draft"), "{case}");
    }
}

#[test]
fn alt_enter_follows_the_link_at_the_caret() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened_plan(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::Windows,
    );

    // Away from links, it does nothing.
    press(&mut harness, Modifiers::NONE, Key::Home);
    press(&mut harness, Modifiers::ALT, Key::Enter);
    assert_eq!(tabs(&harness), [("Plan".to_owned(), true)]);
    assert_eq!(note_text(&harness), PLAN);

    caret_in_draft_link(&mut harness);
    press(&mut harness, Modifiers::ALT, Key::Enter);
    assert_eq!(tabs(&harness), [("Draft".to_owned(), true)]);
}

#[test]
fn typing_two_brackets_lists_files_and_picking_one_inserts_its_link() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened_plan(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::Windows,
    );
    press(&mut harness, Modifiers::NONE, Key::End);

    harness.event(Event::Text(" [[".into()));
    harness.run_steps(3);
    let listed: Vec<String> = harness
        .get_all_by_role(Role::ListBoxOption)
        .map(|option| option.accesskit_node().label().unwrap_or_default())
        .collect();
    assert_eq!(listed, ["Draft", "Other", "Plan"]);

    harness.event(Event::Text("oth".into()));
    harness.run_steps(3);
    harness.get_by_role_and_label(Role::ListBox, "Link suggestions");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    assert_eq!(
        note_text(&harness),
        "See [[Other]] and [the draft](Draft.md). [[Other]]\n"
    );
    assert!(harness.query_by_label("Link suggestions").is_none());

    // A click picks too.
    harness.event(Event::Text(" [[dr".into()));
    harness.run_steps(3);
    harness
        .get_by_role_and_label(Role::ListBoxOption, "Draft")
        .click();
    harness.run_steps(4);
    assert_eq!(
        note_text(&harness),
        "See [[Other]] and [the draft](Draft.md). [[Other]] [[Draft]]\n"
    );

    // Escape closes the list and leaves the text.
    harness.event(Event::Text(" [[pl".into()));
    harness.run_steps(3);
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert!(harness.query_by_label("Link suggestions").is_none());
    assert_eq!(
        note_text(&harness),
        "See [[Other]] and [the draft](Draft.md). [[Other]] [[Draft]] [[pl\n"
    );
}
