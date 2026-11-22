//! Draws the main screens to PNG files, in light and dark, through wgpu
//! and the note editor's own text renderer, to look at the interface
//! without running the app: the folder chooser, a note open beside the
//! sidebar, a row's menu, the command palette, Settings and the Tasks
//! page.
//!
//! It needs a GPU or a software Vulkan driver, so it runs only when asked:
//!
//! ```text
//! NOURA_RENDERS=/tmp/renders cargo test -p app-ui --test it renders:: -- --ignored
//! ```
//!
//! The files land in `NOURA_RENDERS`, or in `noura-renders` in the
//! system's temporary folder. `NOURA_RENDER_SCALE=1` draws them at one
//! pixel per point, as on a display that isn't high-density.

use crate::support;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use app_core::{App, Platform};
use app_ui::{NouraUi, WindowControl, WindowControls};
use egui::{Key, Modifiers, Theme, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable};
use support::{FakePicker, FakeSystem, create_workspace, plugins};

type Ui = Harness<'static, NouraUi>;

const TASKS_MANIFEST: &str = include_str!("../../../plugins/tasks/plugin.toml");

/// The window's size in points.
const SIZE: [f32; 2] = [1200.0, 780.0];

/// Pixels per point: `NOURA_RENDER_SCALE`, or 2 as on a Retina display.
fn scale() -> f32 {
    std::env::var("NOURA_RENDER_SCALE")
        .ok()
        .and_then(|scale| scale.parse().ok())
        .unwrap_or(2.0)
}

const NOTE: &str = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\n\
created: 2026-11-02T10:00:00Z\nupdated: 2026-11-20T16:30:00Z\n---\n\
# Website relaunch\n\n\
The new site goes live with the spring catalogue. This note keeps the plan, \
the open questions and what we decided in Thursday's review, so anyone joining \
can catch up in five minutes.\n\n\
## Goals\n\n\
- Cut the home page to one clear story and a single call to action\n\
- Load in under a second on a phone over 4G\n\
- Move the journal off the old CMS without breaking a single link\n\n\
## Timeline\n\n\
1. Content audit and sitemap, by **November 28**\n\
2. Design review with the studio\n\
3. Build, then a week of *quiet* launch\n\n\
## Open questions\n\n\
- [x] Who owns the photography budget?\n\
- [ ] Do we keep the `/shop` redirects?\n\
- [ ] Which analytics, if any\n\n\
> Ship the smallest site that tells the story well, then listen.\n\n\
### Notes from the review\n\n\
Mara liked the quieter type but asked for more contrast on the captions. \
Theo will prototype the product grid at two densities so we can compare them \
side by side before the next review.\n";

fn note(id: char, title: &str, body: &str) -> String {
    format!(
        "---\nid: note_01jbcdefghjkmnpqrstvwxyz0{id}\ntype: note\n\
         created: 2026-11-02T10:00:00Z\nupdated: 2026-11-20T16:30:00Z\n---\n# {title}\n\n{body}\n"
    )
}

fn task(id: char, properties: &str) -> String {
    format!(
        "---\nid: task_01jbcdefghjkmnpqrstvwxyz0{id}\ntype: task\n{properties}\
         created: 2026-11-01T10:00:00Z\nupdated: 2026-11-01T10:00:00Z\n---\n"
    )
}

/// The fixture workspace: a studio's notes, journal, reading list and
/// tasks.
fn write_workspace(folder: &Path) {
    let write = |path: &str, text: &str| {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("Design/Website relaunch.md", NOTE);
    write(
        "Design/Type specimens.md",
        &note('b', "Type specimens", "Sizes, weights and tracking."),
    );
    write(
        "Design/Brand colors.md",
        &note('c', "Brand colors", "The palette."),
    );
    write(
        "Journal/2026-11-20.md",
        &note('d', "Thursday", "Review day."),
    );
    write("Journal/2026-11-21.md", &note('e', "Friday", "Quiet."));
    write(
        "Reading/The shape of design.md",
        &note('f', "The shape of design", "Frank Chimero."),
    );
    write(
        "Meeting notes.md",
        &note('g', "Meeting notes", "Weekly sync."),
    );
    write(
        "Ideas.md",
        "---\nid: note_01jbcdefghjkmnpqrstvwxyz0h\ntype: note\n\
         created: 2026-11-02T10:00:00Z\nupdated: 2026-11-20T16:30:00Z\n---\n\
         A note with no heading shows its name above the text.\n\n\
         - A quieter empty state for the inbox\n\
         - Keyboard shortcuts on the welcome screen\n",
    );
    write(
        "tasks/Send the sitemap to the studio.md",
        &task('a', "status: todo\npriority: high\ndue: 2026-11-18\n"),
    );
    write(
        "tasks/Book the photographer.md",
        &task('b', "status: todo\npriority: medium\ndue: 2026-11-22\n"),
    );
    write(
        "tasks/Draft the launch email.md",
        &task('c', "status: todo\npriority: low\ndue: 2026-11-26\n"),
    );
    write(
        "tasks/Audit the journal links.md",
        &task('d', "status: todo\npriority: medium\ndue: 2026-12-02\n"),
    );
    write(
        "tasks/Pick an analytics tool.md",
        &task('e', "status: todo\n"),
    );
    write(
        "tasks/Order sample prints.md",
        &task('f', "status: done\ncompleted: 2026-11-19T15:00:00Z\n"),
    );
}

/// Where the renders land.
fn out_dir() -> PathBuf {
    let dir = std::env::var_os("NOURA_RENDERS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("noura-renders"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The buttons a Linux window draws with GNOME's `appmenu:minimize,close`
/// layout, the one the owner's desktop uses.
fn linux_controls() -> WindowControls {
    WindowControls {
        start: vec![],
        end: vec![WindowControl::Minimize, WindowControl::Close],
    }
}

/// A window drawing `app` through wgpu, edge to edge as eframe draws it,
/// with the editor's text renderer registered as the desktop app registers
/// it and the window buttons `controls` lays out.
fn window(app: App, theme: Theme, controls: Option<WindowControls>) -> Ui {
    let render_state = egui_kittest::wgpu::create_render_state(
        egui_kittest::wgpu::default_wgpu_setup(),
        egui_wgpu::RendererOptions::PREDICTABLE,
    );
    let mut fonts = editor_egui::fonts::default_font_database();
    editor_egui::fonts::prefer_sans(&mut fonts, app_ui::fonts::editor_faces());
    // The renderer exists before the window's context, so the font
    // system comes from a context of its own, then moves to the window's.
    let font_system = editor_egui::register_font_system(&egui::Context::default(), fonts);
    editor_egui::register_render_callback_resources(
        &render_state.device,
        &render_state.queue,
        render_state.target_format,
        &mut render_state.renderer.write(),
        Arc::clone(&font_system),
        1,
    );
    let renderer = egui_kittest::wgpu::WgpuTestRenderer::from_render_state(render_state);
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    let mut harness = support::builder()
        .with_size(SIZE)
        .with_pixels_per_point(scale())
        .with_render_cursor(false)
        .with_theme(theme)
        .renderer(renderer)
        .with_context_setup(move |ctx| {
            app_ui::theme::install(ctx);
            editor_egui::install_font_system(ctx, font_system);
        })
        .build_ui_state(support::edge_to_edge, ui);
    harness.state_mut().set_window_controls(controls);
    harness.input_mut().system_theme = Some(theme);
    settle(&mut harness);
    harness
}

/// Run frames, waiting in real time for background work and plugins.
fn settle(harness: &mut Ui) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        harness.step();
        if !harness.state().is_busy() {
            break;
        }
        assert!(Instant::now() < deadline, "background work never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.run_steps(6);
}

fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(harness) {
        assert!(Instant::now() < deadline, "the screen never got there");
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
    }
    settle(harness);
}

fn save(harness: &mut Ui, screen: &str, theme: Theme) {
    let name = match theme {
        Theme::Light => "light",
        Theme::Dark => "dark",
    };
    let image = harness.render().expect("the window renders");
    let path = out_dir().join(format!("{screen}-{name}.png"));
    image.save(&path).unwrap();
    eprintln!("rendered {}", path.display());
}

/// Save the window's title bar row, and the sidebar's first rows under
/// it, as `name`.
fn save_title_row(harness: &mut Ui, name: &str, theme: Theme) {
    let theme = match theme {
        Theme::Light => "light",
        Theme::Dark => "dark",
    };
    let image = harness.render().expect("the window renders");
    let height = (96.0 * scale()) as u32;
    let row = image::imageops::crop_imm(&image, 0, 0, image.width(), height).to_image();
    let path = out_dir().join(format!("{name}-{theme}.png"));
    row.save(&path).unwrap();
    eprintln!("rendered {}", path.display());
}

fn press(harness: &mut Ui, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    settle(harness);
}

#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn the_main_screens_render_in_light_and_dark() {
    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let app = App::launch_with(app_data.path(), FakeSystem::new(Platform::Linux));
        let mut harness = window(app, theme, Some(linux_controls()));
        save(&mut harness, "1-welcome", theme);
    }

    let component = plugins::built("tasks_plugin");
    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Studio");
        create_workspace(app_data.path(), &folder);
        write_workspace(&folder);
        if let Some(component) = &component {
            let plugin = plugins::folder(
                parent.path(),
                TASKS_MANIFEST,
                "tasks_plugin.wasm",
                component,
            );
            plugins::install_and_enable(app_data.path(), &plugin, "tasks");
        }
        let system = FakeSystem::new(Platform::Linux);
        system.set_now("2026-11-22T14:00:00Z");
        let app = App::launch_with(app_data.path(), system);
        let mut harness = window(app, theme, Some(linux_controls()));

        for folder in ["Design", "Journal"] {
            harness
                .get_by_role_and_label(Role::TreeItem, folder)
                .click();
            settle(&mut harness);
        }
        harness
            .get_by_role_and_label(Role::TreeItem, "Website relaunch")
            .click();
        wait_until(&mut harness, |harness| {
            harness
                .query_by_role_and_label(Role::MultilineTextInput, "Note text")
                .is_some()
        });
        save(&mut harness, "2-note", theme);

        // A note with no heading shows its name above its text.
        harness
            .get_by_role_and_label(Role::TreeItem, "Ideas")
            .click();
        settle(&mut harness);
        save(&mut harness, "2b-note-without-heading", theme);
        harness
            .get_by_role_and_label(Role::TreeItem, "Website relaunch")
            .click();
        settle(&mut harness);

        harness
            .get_by_role_and_label(Role::TreeItem, "Type specimens")
            .click_secondary();
        settle(&mut harness);
        save(&mut harness, "3-menu", theme);
        press(&mut harness, Modifiers::NONE, Key::Escape);

        press(&mut harness, Modifiers::COMMAND, Key::K);
        save(&mut harness, "4-palette", theme);
        press(&mut harness, Modifiers::NONE, Key::Escape);

        harness
            .get_by_role_and_label(Role::Button, "Settings")
            .click();
        settle(&mut harness);
        save(&mut harness, "5-settings", theme);
        press(&mut harness, Modifiers::NONE, Key::Escape);

        if component.is_some() {
            harness
                .get_by_role_and_label(Role::Navigation, "Pages")
                .get_by_role_and_label(Role::Link, "Tasks")
                .click();
            wait_until(&mut harness, |harness| {
                harness
                    .query_by_role_and_label(Role::ListBox, "Views")
                    .is_some()
            });
            save(&mut harness, "6-tasks", theme);
        }
    }
}

/// The title bar row of a Linux window with each button layout GNOME
/// offers: its default, the owner's, every button, and buttons at the
/// start.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn the_title_bar_row_renders_with_each_button_layout() {
    use WindowControl::{Close, Maximize, Minimize};
    for (name, start, end) in [
        ("title-row-close", vec![], vec![Close]),
        ("title-row-minimize-close", vec![], vec![Minimize, Close]),
        (
            "title-row-minimize-maximize-close",
            vec![],
            vec![Minimize, Maximize, Close],
        ),
        (
            "title-row-start-close-minimize-maximize",
            vec![Close, Minimize, Maximize],
            vec![],
        ),
    ] {
        for theme in [Theme::Light, Theme::Dark] {
            let app_data = tempfile::tempdir().unwrap();
            let parent = tempfile::tempdir().unwrap();
            let folder = parent.path().join("Documents");
            create_workspace(app_data.path(), &folder);
            write_workspace(&folder);
            let app = App::launch_with(app_data.path(), FakeSystem::new(Platform::Linux));
            let controls = WindowControls {
                start: start.clone(),
                end: end.clone(),
            };
            let mut harness = window(app, theme, Some(controls));
            harness
                .get_by_role_and_label(Role::TreeItem, "Ideas")
                .click();
            settle(&mut harness);
            save_title_row(&mut harness, name, theme);
        }
    }
}
