//! Seam 1: appearance, text size, line width, zoom and spell check, kept
//! for this device in the app data folder.

mod support;

use app_core::{App, Appearance, LineWidth, Preferences, TextSize};
use support::{Fixture, settle};

/// Quit by dropping `app`, then launch again on the same app data.
fn relaunch(app: App, fixture: &Fixture) -> App {
    drop(app);
    let mut app = App::launch(fixture.app_data.path());
    settle(&mut app);
    app
}

/// Every file and folder in the workspace with each file's bytes, then
/// the manifest's bytes.
type WorkspaceBytes = (Vec<(String, Option<Vec<u8>>)>, Vec<u8>);

fn workspace_bytes(fixture: &Fixture) -> WorkspaceBytes {
    (fixture.snapshot(), fixture.read(".noura/workspace.yaml"))
}

#[test]
fn a_new_device_matches_the_system_theme_at_default_text_size_and_full_zoom() {
    let fixture = Fixture::new(&[]);
    let app = fixture.app();

    assert_eq!(
        *app.preferences(),
        Preferences {
            appearance: Appearance::MatchSystem,
            text_size: TextSize::Default,
            line_width: LineWidth::Default,
            zoom: 100,
            spell_check: true,
        }
    );
    assert_eq!(app.preferences().zoom_factor(), 1.0);
}

#[test]
fn appearance_and_text_size_survive_a_relaunch_and_leave_the_workspace_alone() {
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    let before = workspace_bytes(&fixture);

    app.set_appearance(Appearance::Dark);
    app.set_text_size(TextSize::Large);
    let mut app = relaunch(app, &fixture);
    assert_eq!(app.preferences().appearance, Appearance::Dark);
    assert_eq!(app.preferences().text_size, TextSize::Large);

    app.set_appearance(Appearance::Light);
    app.set_text_size(TextSize::Small);
    let mut app = relaunch(app, &fixture);
    assert_eq!(app.preferences().appearance, Appearance::Light);
    assert_eq!(app.preferences().text_size, TextSize::Small);

    app.set_appearance(Appearance::MatchSystem);
    let app = relaunch(app, &fixture);
    assert_eq!(app.preferences().appearance, Appearance::MatchSystem);
    assert_eq!(workspace_bytes(&fixture), before);
}

#[test]
fn line_width_survives_a_relaunch_and_leaves_the_workspace_alone() {
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    let before = workspace_bytes(&fixture);

    for width in [LineWidth::Narrow, LineWidth::Full, LineWidth::Default] {
        app.set_line_width(width);
        app = relaunch(app, &fixture);
        assert_eq!(app.preferences().line_width, width);
    }
    assert_eq!(workspace_bytes(&fixture), before);
    let layout =
        std::fs::read_to_string(fixture.app_data.path().join("layout.json")).unwrap_or_default();
    assert!(
        !layout.contains("lineWidth"),
        "the line width lives with the other preferences: {layout}"
    );
}

#[test]
fn zoom_steps_through_the_browser_levels_and_stops_at_each_end() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();

    let mut zoomed_in = Vec::new();
    for _ in 0..6 {
        app.zoom_in();
        zoomed_in.push(app.preferences().zoom);
    }
    assert_eq!(zoomed_in, [110, 125, 150, 175, 200, 200]);

    app.actual_size();
    assert_eq!(app.preferences().zoom, 100);
    let mut zoomed_out = Vec::new();
    for _ in 0..6 {
        app.zoom_out();
        zoomed_out.push(app.preferences().zoom);
    }
    assert_eq!(zoomed_out, [90, 80, 75, 67, 50, 50]);
    assert_eq!(app.preferences().zoom_factor(), 0.5);
}

#[test]
fn zoom_survives_a_relaunch_and_leaves_the_workspace_alone() {
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    let before = workspace_bytes(&fixture);

    app.zoom_in();
    app.zoom_in();
    let mut app = relaunch(app, &fixture);
    assert_eq!(app.preferences().zoom, 125);

    app.actual_size();
    let app = relaunch(app, &fixture);
    assert_eq!(app.preferences().zoom, 100);
    assert_eq!(workspace_bytes(&fixture), before);
}

#[test]
fn an_unreadable_preferences_file_falls_back_to_the_defaults_and_launch_goes_on() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    app.set_appearance(Appearance::Dark);
    app.zoom_in();
    drop(app);
    let file = fixture.app_data.path().join("preferences.json");
    assert!(
        file.is_file(),
        "the preferences live in the app data folder"
    );

    for broken in [
        &b"\xff\xfe not json"[..],
        b"{\"appearance\": \"purple\", \"textSize\": 3, \"lineWidth\": 1, \"zoom\": 333}",
        b"[]",
    ] {
        std::fs::write(&file, broken).unwrap();
        let mut app = App::launch(fixture.app_data.path());
        settle(&mut app);

        assert!(app.workspace().is_some(), "the workspace didn't reopen");
        assert_eq!(*app.preferences(), Preferences::default());
    }

    // A folder where the file belongs can't be read or replaced, yet the
    // app starts and the choice applies until it quits.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    let mut app = App::launch(fixture.app_data.path());
    settle(&mut app);
    assert!(app.workspace().is_some(), "the workspace didn't reopen");
    assert_eq!(*app.preferences(), Preferences::default());
    app.set_text_size(TextSize::Large);
    assert_eq!(app.preferences().text_size, TextSize::Large);
}

#[test]
fn one_unreadable_choice_falls_back_alone() {
    let fixture = Fixture::new(&[]);
    drop(fixture.app());
    let file = fixture.app_data.path().join("preferences.json");
    std::fs::write(
        &file,
        b"{\"appearance\": \"dark\", \"textSize\": \"huge\", \"lineWidth\": \"narrow\", \"zoom\": 150}",
    )
    .unwrap();

    let mut app = App::launch(fixture.app_data.path());
    settle(&mut app);

    assert_eq!(
        *app.preferences(),
        Preferences {
            appearance: Appearance::Dark,
            text_size: TextSize::Default,
            line_width: LineWidth::Narrow,
            zoom: 150,
            spell_check: true,
        }
    );
}

#[test]
fn turning_spell_check_off_survives_a_relaunch_and_leaves_the_workspace_alone() {
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    let before = workspace_bytes(&fixture);
    assert!(
        app.preferences().spell_check,
        "a new device checks spelling"
    );

    app.set_spell_check(false);
    let mut app = relaunch(app, &fixture);
    assert!(!app.preferences().spell_check);

    app.set_spell_check(true);
    let app = relaunch(app, &fixture);
    assert!(app.preferences().spell_check);
    assert_eq!(workspace_bytes(&fixture), before);
}

#[test]
fn an_unreadable_spell_check_choice_falls_back_to_checking() {
    let fixture = Fixture::new(&[]);
    drop(fixture.app());
    let file = fixture.app_data.path().join("preferences.json");
    std::fs::write(&file, b"{\"zoom\": 125, \"spellCheck\": \"no\"}").unwrap();

    let mut app = App::launch(fixture.app_data.path());
    settle(&mut app);

    assert!(app.preferences().spell_check);
    assert_eq!(app.preferences().zoom, 125);
}
