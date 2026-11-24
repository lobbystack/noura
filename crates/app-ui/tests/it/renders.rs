//! Draws the main screens to PNG files, in light and dark, through wgpu
//! and the note editor's own text renderer, to look at the interface
//! without running the app: the folder chooser, a note open beside the
//! sidebar, a row's menu, the command palette, Settings and the Tasks
//! page, and the Calendar page's month, week and day at two window sizes.
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
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable},
};
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

/// Save the part of the window `rect` covers, in points, as `name`.
fn save_part(harness: &mut Ui, name: &str, rect: egui::Rect) {
    let image = harness.render().expect("the window renders");
    let scale = scale();
    let x = (rect.min.x * scale).max(0.0) as u32;
    let y = (rect.min.y * scale).max(0.0) as u32;
    let width = ((rect.width() * scale) as u32).min(image.width().saturating_sub(x));
    let height = ((rect.height() * scale) as u32).min(image.height().saturating_sub(y));
    let part = image::imageops::crop_imm(&image, x, y, width, height).to_image();
    let path = out_dir().join(format!("{name}.png"));
    part.save(&path).unwrap();
    eprintln!("rendered {}", path.display());
}

/// Each PDF texture's name and size in pixels, as uploaded.
fn pdf_textures(harness: &Ui) -> Vec<(String, [usize; 2])> {
    let textures = harness.ctx.tex_manager();
    let textures = textures.read();
    let mut found: Vec<(String, [usize; 2])> = textures
        .allocated()
        .filter(|(_, meta)| meta.name.starts_with("pdf"))
        .map(|(_, meta)| (meta.name.clone(), meta.size))
        .collect();
    found.sort();
    found
}

/// A page 612 by 14,000 points, as a web page saved as PDF, fit to the
/// tab's width at two pixels a point: the window, the top of the page at
/// one window pixel per screen pixel, and the size of each texture the
/// page drew against the size it shows at.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn a_tall_pdf_page_renders_at_the_screens_density() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Reading");
    create_workspace(app_data.path(), &folder);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app-core/tests/fixtures/pdf");
    std::fs::copy(fixture.join("tall.pdf"), folder.join("tall.pdf")).unwrap();
    let app = App::launch_with(app_data.path(), FakeSystem::new(Platform::Linux));
    let mut harness = window(app, Theme::Light, Some(linux_controls()));
    // The desktop app's GPU limit, past kittest's 2048.
    harness.input_mut().max_texture_side = Some(8192);

    harness
        .get_by_role_and_label(Role::TreeItem, "tall.pdf")
        .click();
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Image, "Page 1 of 2")
            .is_some()
    });
    // Let every drawing the view asked for arrive.
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        harness.step();
    }
    settle(&mut harness);
    save(&mut harness, "7-pdf-tall", Theme::Light);
    let page = harness
        .get_by_role_and_label(Role::Image, "Page 1 of 2")
        .rect();
    let shown = page.intersect(harness.ctx.viewport_rect());
    let top = egui::Rect::from_min_size(shown.min, egui::vec2(shown.width().min(400.0), 160.0));
    save_part(&mut harness, "7-pdf-tall-top", top);
    eprintln!(
        "page shows at {:?}, {} by {} points, {} pixels a point",
        page.min,
        page.width(),
        page.height(),
        scale()
    );
    for (name, [width, height]) in pdf_textures(&harness) {
        eprintln!(
            "texture {name}: {width} by {height} pixels, {:.3} pixels a point across",
            width as f32 / page.width()
        );
    }

    // Find scrolls 12,800 points down the page to line 800.
    let field = harness.get_by_role_and_label(Role::TextInput, "Search PDF");
    field.focus();
    field.type_text("needle");
    wait_until(&mut harness, |harness| {
        harness.query_by_label("1 of 1").is_some()
    });
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        harness.step();
    }
    settle(&mut harness);
    save(&mut harness, "7-pdf-tall-needle", Theme::Light);
    let pages = harness
        .get_by_role_and_label(Role::ScrollView, "Pages")
        .rect();
    let line = egui::Rect::from_min_size(
        pages.min + egui::vec2(0.0, pages.height() / 3.0 - 40.0),
        egui::vec2(pages.width().min(400.0), 120.0),
    );
    save_part(&mut harness, "7-pdf-tall-needle-line", line);
    for (name, [width, height]) in pdf_textures(&harness) {
        eprintln!(
            "after find: texture {name}: {width} by {height} pixels, {:.3} pixels a point across",
            width as f32 / page.width()
        );
    }
}

/// A note that embeds the page 612 by 14,000 points, at two pixels a
/// point: the window, the top of the embed at one window pixel per screen
/// pixel, and the size of each texture the embed drew against the size it
/// shows at.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn a_tall_pdf_page_embedded_in_a_note_renders_at_the_screens_density() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Reading");
    create_workspace(app_data.path(), &folder);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app-core/tests/fixtures/pdf");
    std::fs::copy(fixture.join("tall.pdf"), folder.join("tall.pdf")).unwrap();
    std::fs::write(
        folder.join("Saved page.md"),
        note(
            'a',
            "Saved page",
            "The article, as saved.\n\n![[tall.pdf]]\n",
        ),
    )
    .unwrap();
    let app = App::launch_with(app_data.path(), FakeSystem::new(Platform::Linux));
    let mut harness = window(app, Theme::Light, Some(linux_controls()));
    // The desktop app's GPU limit, past kittest's 2048.
    harness.input_mut().max_texture_side = Some(8192);

    harness
        .get_by_role_and_label(Role::TreeItem, "Saved page")
        .click();
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Button, "Open tall.pdf in a tab")
            .is_some()
    });
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        harness.step();
    }
    settle(&mut harness);
    save(&mut harness, "8-pdf-embed-tall", Theme::Light);
    let embed = harness
        .get_by_role_and_label(Role::Image, "tall.pdf")
        .rect();
    let shown = embed.intersect(harness.ctx.viewport_rect());
    let top = egui::Rect::from_min_size(shown.min, egui::vec2(shown.width().min(400.0), 160.0));
    save_part(&mut harness, "8-pdf-embed-tall-top", top);
    eprintln!(
        "embed shows at {:?}, {} by {} points, {} pixels a point",
        embed.min,
        embed.width(),
        embed.height(),
        scale()
    );
    for (name, [width, height]) in pdf_textures(&harness) {
        eprintln!(
            "texture {name}: {width} by {height} pixels, {:.3} pixels a point across",
            width as f32 / embed.width()
        );
    }

    // Scroll the note thousands of points down the page.
    let view = harness.ctx.viewport_rect();
    harness.hover_at(egui::pos2(view.center().x, view.center().y + 100.0));
    for _ in 0..12 {
        harness.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -500.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(50));
        harness.step();
    }
    settle(&mut harness);
    save(&mut harness, "8-pdf-embed-tall-far", Theme::Light);
    let embed = harness
        .get_by_role_and_label(Role::Image, "tall.pdf")
        .rect();
    let shown = embed.intersect(harness.ctx.viewport_rect());
    let part = egui::Rect::from_min_size(shown.min, egui::vec2(shown.width().min(400.0), 160.0));
    save_part(&mut harness, "8-pdf-embed-tall-far-part", part);
    eprintln!(
        "far down: embed shows at {:?}, {} by {} points",
        embed.min,
        embed.width(),
        embed.height()
    );
    for (name, [width, height]) in pdf_textures(&harness) {
        eprintln!(
            "far down: texture {name}: {width} by {height} pixels, {:.3} pixels a point across",
            width as f32 / embed.width()
        );
    }
    let tab = harness.state().app().active_tab().unwrap().id();
    let links = harness.state().app().note_links(tab).unwrap();
    eprintln!(
        "far down: app-core keeps {} bytes of embed tiles",
        links.pdf_embed_tile_bytes_for_testing()
    );
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

const CALENDAR_MANIFEST: &str = include_str!("../../../plugins/calendar/plugin.toml");
const PROJECTS_MANIFEST: &str = include_str!("../../../plugins/projects/plugin.toml");

/// The plugin pages' headers: Calendar's month in Month, Week and Day,
/// Tasks and Projects, each with its buttons, in light and dark.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn the_plugin_page_headers_render() {
    let built = [
        ("tasks", TASKS_MANIFEST, "tasks_plugin"),
        ("calendar", CALENDAR_MANIFEST, "calendar_plugin"),
        ("projects", PROJECTS_MANIFEST, "projects_plugin"),
    ]
    .map(|(id, manifest, name)| (id, manifest, name, plugins::built(name)));
    if built.iter().any(|(.., component)| component.is_none()) {
        return;
    }
    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Studio");
        create_workspace(app_data.path(), &folder);
        write_workspace(&folder);
        for (id, manifest, name, component) in &built {
            let plugin = plugins::folder(
                &parent.path().join(id),
                manifest,
                &format!("{name}.wasm"),
                component.as_ref().unwrap(),
            );
            plugins::install_and_enable(app_data.path(), &plugin, id);
        }
        let system = FakeSystem::new(Platform::Linux);
        system.set_now("2026-11-22T14:00:00Z");
        let app = App::launch_with(app_data.path(), system);
        let mut harness = window(app, theme, Some(linux_controls()));
        for (page, ready, name) in [
            ("Calendar", "Today", "calendar-month"),
            ("Tasks", "Filters", "tasks"),
            ("Projects", "New project", "projects"),
        ] {
            harness
                .get_by_role_and_label(Role::Navigation, "Pages")
                .get_by_role_and_label(Role::Link, page)
                .click();
            wait_until(&mut harness, |harness| {
                harness
                    .query_by_role_and_label(Role::Button, ready)
                    .is_some()
            });
            save(&mut harness, name, theme);
            if page == "Calendar" {
                for mode in ["Week", "Day"] {
                    harness
                        .get_by_role_and_label(Role::RadioButton, mode)
                        .click();
                    settle(&mut harness);
                    save(
                        &mut harness,
                        &format!("calendar-{}", mode.to_lowercase()),
                        theme,
                    );
                }
                harness
                    .get_by_role_and_label(Role::RadioButton, "Month")
                    .click();
                settle(&mut harness);
            }
        }
    }
}

/// The calendars of the calendar fixture: (ID, name, color).
const CALENDARS: [(&str, &str, &str); 4] = [
    ("calendar_01jbcdefghjkmnpqrstvwxyz0a", "Personal", "purple"),
    ("calendar_01jbcdefghjkmnpqrstvwxyz0b", "Family", "yellow"),
    ("calendar_01jbcdefghjkmnpqrstvwxyz0c", "Work", "blue"),
    ("calendar_01jbcdefghjkmnpqrstvwxyz0d", "Holidays", "red"),
];

/// The events of the calendar fixture, in Toronto's time in November:
/// (calendar, title, start, end). A date start makes an all-day event.
const EVENTS: [(usize, &str, &str, &str); 30] = [
    (
        2,
        "Bring the laptop in",
        "2026-11-04T09:00",
        "2026-11-04T09:30",
    ),
    (2, "Studio meeting", "2026-11-04T14:00", "2026-11-04T15:00"),
    (2, "Course sign-up", "2026-11-05T10:00", "2026-11-05T10:30"),
    (
        0,
        "Pottery class sign-up",
        "2026-11-05T18:00",
        "2026-11-05T18:30",
    ),
    (
        2,
        "Finish the Figma mockups",
        "2026-11-08T13:00",
        "2026-11-08T16:00",
    ),
    (
        2,
        "Ideation workshop",
        "2026-11-10T09:00",
        "2026-11-10T12:00",
    ),
    (3, "Remembrance Day", "2026-11-11", "2026-11-12"),
    (0, "Coffee with Sam", "2026-11-13T10:00", "2026-11-13T11:00"),
    (
        2,
        "Prepare the interview",
        "2026-11-16T15:00",
        "2026-11-16T16:00",
    ),
    (
        0,
        "Remote management class",
        "2026-11-19T18:00",
        "2026-11-19T20:00",
    ),
    (2, "Solo work block", "2026-11-20T09:00", "2026-11-20T12:00"),
    (1, "Brunch at Mom's", "2026-11-22T11:00", "2026-11-22T13:00"),
    (2, "Interview", "2026-11-23T09:00", "2026-11-23T10:00"),
    (2, "Design review", "2026-11-23T09:30", "2026-11-23T11:00"),
    (0, "Dentist", "2026-11-23T12:30", "2026-11-23T13:30"),
    (
        0,
        "Call the plumber",
        "2026-11-23T15:00",
        "2026-11-23T15:30",
    ),
    (0, "Gym", "2026-11-23T17:30", "2026-11-23T18:30"),
    (1, "Dinner with Ana", "2026-11-23T19:00", "2026-11-23T21:00"),
    (1, "Mom's birthday", "2026-11-25", "2026-11-26"),
    (0, "Cabin trip", "2026-11-26", "2026-11-29"),
    (
        2,
        "Quarterly planning",
        "2026-11-27T10:00",
        "2026-11-27T12:00",
    ),
    (1, "Dad's party", "2026-11-29", "2026-11-30"),
    (
        2,
        "Final presentation",
        "2026-12-01T10:00",
        "2026-12-01T11:00",
    ),
    (
        2,
        "Project report due",
        "2026-12-02T17:00",
        "2026-12-02T17:30",
    ),
    (
        2,
        "Presentation rehearsal",
        "2026-12-03T09:00",
        "2026-12-03T10:00",
    ),
    (
        0,
        "Pay the hydro bill",
        "2026-12-03T12:00",
        "2026-12-03T12:30",
    ),
    (1, "Dinner with Dad", "2026-12-04T18:30", "2026-12-04T20:30"),
    (2, "Team project", "2026-12-05T10:00", "2026-12-05T12:00"),
    (2, "Typography exam", "2026-12-08T09:00", "2026-12-08T12:00"),
    (
        2,
        "Interaction exam",
        "2026-12-09T09:00",
        "2026-12-09T12:00",
    ),
];

/// The calendar fixture: four calendars and a month of events, with
/// all-day and multi-day events and a day with more events than a month's
/// cell shows, beside the studio's tasks.
fn write_calendar_workspace(folder: &Path) {
    write_workspace(folder);
    let write = |path: &str, text: &str| {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    for (id, name, color) in CALENDARS {
        write(
            &format!("Calendars/{name}.md"),
            &format!("---\nid: {id}\ntype: calendar\ncolor: {color}\n---\n"),
        );
    }
    for (index, (calendar, title, start, end)) in EVENTS.into_iter().enumerate() {
        let time = |at: &str| {
            if at.contains('T') {
                format!("{at}:00-05:00")
            } else {
                at.to_owned()
            }
        };
        write(
            &format!("Events/{title}.md"),
            &format!(
                "---\nid: event_01jbcdefghjkmnpqrstvw{index:05}\ntype: event\n\
                 calendar: {}\nstart: {}\nend: {}\n---\n",
                CALENDARS[calendar].0,
                time(start),
                time(end)
            ),
        );
    }
}

/// The Calendar page in month, week and day, in a 1200 by 780 window and
/// a tall 1000 by 1000 one, in light and dark, named
/// `calendar-<mode>-<width>x<height>`.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn the_calendar_renders_each_mode_at_two_sizes() {
    let (Some(calendar), Some(tasks)) = (
        plugins::built("calendar_plugin"),
        plugins::built("tasks_plugin"),
    ) else {
        return;
    };
    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Studio");
        create_workspace(app_data.path(), &folder);
        write_calendar_workspace(&folder);
        for (id, manifest, entry, component) in [
            ("tasks", TASKS_MANIFEST, "tasks_plugin.wasm", &tasks),
            (
                "calendar",
                CALENDAR_MANIFEST,
                "calendar_plugin.wasm",
                &calendar,
            ),
        ] {
            let place = parent.path().join(format!("{id}-plugin"));
            let plugin = plugins::folder(&place, manifest, entry, component);
            plugins::install_and_enable(app_data.path(), &plugin, id);
        }
        let system = FakeSystem::new(Platform::Linux);
        system.set_now("2026-11-22T14:00:00Z");
        let app = App::launch_with(app_data.path(), system);
        let mut harness = window(app, theme, Some(linux_controls()));
        harness
            .get_by_role_and_label(Role::Navigation, "Pages")
            .get_by_role_and_label(Role::Link, "Calendar")
            .click();
        wait_until(&mut harness, |harness| {
            harness
                .query_by_role_and_label(Role::Grid, "Calendar")
                .is_some()
        });
        for mode in ["Month", "Week", "Day"] {
            harness
                .get_by_role_and_label(Role::RadioButton, mode)
                .click();
            settle(&mut harness);
            if mode == "Day" {
                // The day with the most events.
                harness
                    .get_by_role_and_label(Role::Button, "Next day")
                    .click();
                settle(&mut harness);
            }
            for size in [SIZE, [1000.0, 1000.0]] {
                harness.set_size(egui::vec2(size[0], size[1]));
                settle(&mut harness);
                let name = format!(
                    "calendar-{}-{}x{}",
                    mode.to_lowercase(),
                    size[0] as u32,
                    size[1] as u32
                );
                save(&mut harness, &name, theme);
            }
            harness.set_size(egui::vec2(SIZE[0], SIZE[1]));
            settle(&mut harness);
        }
    }
}

/// The Files tab with a folder's files, an empty folder and every tab
/// closed, then the tab bar with one tab, a preview beside kept tabs and
/// a full row, in light and dark. Every step is a click, as a person
/// would take it.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn the_files_tab_and_the_tab_bar_render() {
    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Studio");
        create_workspace(app_data.path(), &folder);
        write_workspace(&folder);
        std::fs::create_dir_all(folder.join("Archive")).unwrap();
        let app = App::launch_with(app_data.path(), FakeSystem::new(Platform::Linux));
        let mut harness = window(app, theme, Some(linux_controls()));

        let rail = |harness: &mut Ui, page: &str, modifiers: Modifiers| {
            harness
                .get_by_role_and_label(Role::Navigation, "Pages")
                .get_by_role_and_label(Role::Link, page)
                .click_modifiers(modifiers);
            settle(harness);
        };
        rail(&mut harness, "Files", Modifiers::NONE);
        save(&mut harness, "files-tab", theme);
        if let Some(list) = harness.query_by_role_and_label(Role::ListBox, "Files") {
            list.get_by_role_and_label(Role::ListBoxOption, "Archive")
                .click();
            settle(&mut harness);
            save(&mut harness, "files-tab-empty-folder", theme);
            harness
                .get_by_role_and_label(Role::Navigation, "Path")
                .get_by_role_and_label(Role::Button, "Studio")
                .click();
            settle(&mut harness);
        }
        loop {
            let close = harness
                .query_all_by_role(Role::Button)
                .filter_map(|node| node.accesskit_node().label())
                .find(|label| label.starts_with("Close "));
            let Some(close) = close else {
                break;
            };
            harness.get_by_role_and_label(Role::Button, &close).click();
            settle(&mut harness);
        }
        save(&mut harness, "all-tabs-closed", theme);

        let tree = |harness: &mut Ui, name: &str, modifiers: Modifiers| {
            harness
                .get_by_role_and_label(Role::TreeItem, name)
                .click_modifiers(modifiers);
            settle(harness);
        };
        for folder in ["Design", "Journal", "tasks"] {
            tree(&mut harness, folder, Modifiers::NONE);
        }
        tree(&mut harness, "Website relaunch", Modifiers::COMMAND);
        save(&mut harness, "tabs-one", theme);
        tree(&mut harness, "Type specimens", Modifiers::COMMAND);
        tree(&mut harness, "Brand colors", Modifiers::NONE);
        save(&mut harness, "tabs-preview-and-kept", theme);
        for name in [
            "Ideas",
            "Meeting notes",
            "2026-11-20",
            "2026-11-21",
            "Book the photographer",
            "Draft the launch email",
            "Order sample prints",
            "Pick an analytics tool",
        ] {
            tree(&mut harness, name, Modifiers::COMMAND);
        }
        save(&mut harness, "tabs-full-row", theme);
    }
}

/// Calendar's sidebar with Files in every sidebar off, as at first, and
/// on, in light and dark.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn the_calendar_sidebar_renders_with_and_without_files() {
    let Some(component) = plugins::built("calendar_plugin") else {
        return;
    };
    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Studio");
        create_workspace(app_data.path(), &folder);
        write_calendar_workspace(&folder);
        let plugin = plugins::folder(
            &parent.path().join("calendar"),
            CALENDAR_MANIFEST,
            "calendar_plugin.wasm",
            &component,
        );
        plugins::install_and_enable(app_data.path(), &plugin, "calendar");
        let system = FakeSystem::new(Platform::Linux);
        system.set_now("2026-11-22T14:00:00Z");
        let app = App::launch_with(app_data.path(), system);
        let mut harness = window(app, theme, Some(linux_controls()));
        harness
            .get_by_role_and_label(Role::Navigation, "Pages")
            .get_by_role_and_label(Role::Link, "Calendar")
            .click();
        wait_until(&mut harness, |harness| {
            harness
                .query_by_role_and_label(Role::Grid, "Calendar")
                .is_some()
        });
        save(&mut harness, "calendar-sidebar", theme);
        // Turn Files in every sidebar on, where Settings has it.
        harness
            .get_by_role_and_label(Role::Button, "Settings")
            .click();
        settle(&mut harness);
        let switch = harness.query_by_role_and_label(Role::Switch, "Files in every sidebar");
        let Some(switch) = switch else {
            continue;
        };
        switch.click();
        settle(&mut harness);
        harness.key_press(egui::Key::Escape);
        settle(&mut harness);
        save(&mut harness, "calendar-sidebar-with-files", theme);
    }
}
