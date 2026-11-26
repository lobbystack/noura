//! Draws the main screens to PNG files, in light and dark, through wgpu
//! and the note editor's own text renderer, to look at the interface
//! without running the app: the folder chooser, a note open beside the
//! sidebar, a row's menu, the command palette, Settings and the Tasks
//! page, and the Calendar page's month, week and day at two window sizes.
//! `website_screens` and `website_parts` draw the screenshots and the
//! pieces of the interface `scripts/website-screens.sh` turns into the
//! website's images.
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
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use app_core::{
    App, Platform,
    ai_client::{
        AiProviderEventStream, AiProviderStreamEvent, AiStreamer, AiToolCall,
        ResolvedAiStreamRequest,
    },
};
use app_ui::{NouraUi, WindowControl, WindowControls};
use egui::{Key, Modifiers, Theme, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable},
};
use support::{FakePicker, FakeSystem, create_workspace, plugins};

type Ui = Harness<'static, NouraUi>;

/// A plugin Noura ships: its ID, its manifest, and its crate's library
/// name, which names its built component.
type Shipped = (&'static str, &'static str, &'static str);

const TASKS: Shipped = (
    "tasks",
    include_str!("../../../plugins/tasks/plugin.toml"),
    "tasks_plugin",
);
const CALENDAR: Shipped = (
    "calendar",
    include_str!("../../../plugins/calendar/plugin.toml"),
    "calendar_plugin",
);
const PROJECTS: Shipped = (
    "projects",
    include_str!("../../../plugins/projects/plugin.toml"),
    "projects_plugin",
);
const AI: Shipped = (
    "ai",
    include_str!("../../../plugins/ai/plugin.toml"),
    "ai_plugin",
);

const DICTATION: Shipped = (
    "dictation",
    include_str!("../../../plugins/dictation/plugin.toml"),
    "dictation_plugin",
);
const TRANSCRIPTION: Shipped = (
    "transcription",
    include_str!("../../../plugins/transcription/plugin.toml"),
    "transcription_plugin",
);

/// Install each of `shipped` from a folder in `parent` and turn it on in
/// the workspace `app_data` opens at launch. False, with none installed,
/// when one of them isn't built.
fn install(app_data: &Path, parent: &Path, shipped: &[Shipped]) -> bool {
    install_turned(app_data, parent, shipped, true)
}

/// As [`install`], with each plugin on or off as `on` says.
fn install_turned(app_data: &Path, parent: &Path, shipped: &[Shipped], on: bool) -> bool {
    let Some(components) = shipped
        .iter()
        .map(|(.., name)| plugins::built(name))
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    for ((id, manifest, name), component) in shipped.iter().zip(components) {
        let place = parent.join(format!("{id}-plugin"));
        let plugin = plugins::folder(&place, manifest, &format!("{name}.wasm"), &component);
        plugins::install_turned(app_data, &plugin, id, on);
    }
    true
}

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
- Move the journal off the old CMS without breaking a single link\n\
- Give every catalogue photo real alt text\n\n\
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

fn task(id: char, properties: &str, body: &str) -> String {
    format!(
        "---\nid: task_01jbcdefghjkmnpqrstvwxyz0{id}\ntype: task\n{properties}\
         created: 2026-11-01T10:00:00Z\nupdated: 2026-11-01T10:00:00Z\n---\n{body}"
    )
}

/// The fixture workspace: a studio's notes, journal, reading list,
/// projects and tasks.
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
        "Projects/Studio admin/Studio admin.md",
        "---\nid: project_01jbcdefghjkmnpqrstvwxyz0b\ntype: project\nstatus: active\n\
         created: 2026-09-01T10:00:00Z\nupdated: 2026-11-19T15:00:00Z\n---\n\
         Invoices, renewals and supplies.\n",
    );
    write(
        "Projects/Spring launch/Spring launch.md",
        "---\nid: project_01jbcdefghjkmnpqrstvwxyz0a\ntype: project\nstatus: active\n\
         created: 2026-10-20T10:00:00Z\nupdated: 2026-11-20T16:30:00Z\n---\n\
         The new website, live with the spring catalogue.\n",
    );
    for (id, title, properties, body) in STUDIO_TASKS {
        write(&format!("Tasks/{title}.md"), &task(id, properties, body));
    }
}

/// The studio's tasks: (ID suffix, title, properties, body).
const STUDIO_TASKS: [(char, &str, &str, &str); 15] = [
    (
        'a',
        "Send the sitemap to the studio",
        "status: todo\npriority: high\ndue: 2026-11-18\nproject: '[[Spring launch]]'\n",
        "Final sitemap from the content audit, with the journal's sections.\n",
    ),
    (
        'b',
        "Book the photographer",
        "status: todo\npriority: medium\ndue: 2026-11-22\nproject: '[[Spring launch]]'\n",
        "",
    ),
    (
        'c',
        "Draft the launch email",
        "status: todo\npriority: low\ndue: 2026-11-26\nproject: '[[Spring launch]]'\n",
        "",
    ),
    (
        'd',
        "Audit the journal links",
        "status: todo\npriority: medium\nassignee: Mara\ndue: 2026-12-02\nproject: '[[Spring launch]]'\n",
        "",
    ),
    (
        'e',
        "Pick an analytics tool",
        "status: todo\ndue: 2026-11-27\nproject: '[[Spring launch]]'\n",
        "",
    ),
    (
        'f',
        "Order sample prints",
        "status: done\ncompleted: 2026-11-19T15:00:00Z\nproject: '[[Studio admin]]'\n",
        "",
    ),
    (
        'g',
        "Prototype the product grid",
        "status: in-progress\npriority: high\nassignee: Theo\ndue: 2026-11-23\nproject: '[[Spring launch]]'\n",
        "Three and four columns on the catalogue page, with and without prices \
         under the photos. Print both for Monday's review.\n",
    ),
    (
        'h',
        "Raise the contrast on the captions",
        "status: todo\npriority: medium\nassignee: Mara\ndue: 2026-11-24\nproject: '[[Spring launch]]'\n",
        "Mara's note from the review: the grey captions fail on the cream background.\n",
    ),
    (
        'j',
        "Renew the studio domain",
        "status: todo\npriority: urgent\ndue: 2026-11-25\nproject: '[[Studio admin]]'\n",
        "",
    ),
    (
        'k',
        "Write alt text for the catalogue photos",
        "status: todo\npriority: low\ndue: 2026-11-30\nproject: '[[Spring launch]]'\n",
        "",
    ),
    (
        'm',
        "Invoice the November retainer",
        "status: todo\npriority: medium\ndue: 2026-11-30\nproject: '[[Studio admin]]'\n",
        "",
    ),
    (
        'n',
        "Map the old journal URLs to the new ones",
        "status: todo\npriority: medium\ndue: 2026-12-04\nproject: '[[Spring launch]]'\n",
        "",
    ),
    (
        'p',
        "Finish the content audit",
        "status: done\npriority: high\ncompleted: 2026-11-20T17:00:00Z\nproject: '[[Spring launch]]'\n",
        "",
    ),
    (
        'q',
        "Export the brand colors for the developers",
        "status: done\ncompleted: 2026-11-21T11:00:00Z\n",
        "",
    ),
    (
        'r',
        "Back up the photo archive",
        "status: todo\npriority: low\ndue: 2026-12-01\nproject: '[[Studio admin]]'\n",
        "",
    ),
];

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
    let image = harness.render().expect("the window renders");
    write_png(&image, &format!("{screen}-{}", theme_name(theme)));
}

/// The name a render in `theme` ends with.
fn theme_name(theme: Theme) -> &'static str {
    match theme {
        Theme::Light => "light",
        Theme::Dark => "dark",
    }
}

/// Write `image` to `name.png` among the renders.
fn write_png(image: &image::RgbaImage, name: &str) {
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).unwrap();
    eprintln!("rendered {}", path.display());
}

/// Save the window's title bar row, and the sidebar's first rows under
/// it, as `name`.
fn save_title_row(harness: &mut Ui, name: &str, theme: Theme) {
    let theme = theme_name(theme);
    let image = harness.render().expect("the window renders");
    let height = (96.0 * scale()) as u32;
    let row = image::imageops::crop_imm(&image, 0, 0, image.width(), height).to_image();
    write_png(&row, &format!("{name}-{theme}"));
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

    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Studio");
        create_workspace(app_data.path(), &folder);
        write_workspace(&folder);
        let tasks = install(app_data.path(), parent.path(), &[TASKS]);
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

        if tasks {
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
    write_png(&part, name);
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

/// The plugin pages' headers: Calendar's month in Month, Week and Day,
/// Tasks and Projects, each with its buttons, in light and dark.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn the_plugin_page_headers_render() {
    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Studio");
        create_workspace(app_data.path(), &folder);
        write_workspace(&folder);
        if !install(app_data.path(), parent.path(), &[TASKS, CALENDAR, PROJECTS]) {
            return;
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
    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Studio");
        create_workspace(app_data.path(), &folder);
        write_calendar_workspace(&folder);
        if !install(app_data.path(), parent.path(), &[TASKS, CALENDAR]) {
            return;
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
        close_tabs(&mut harness);
        save(&mut harness, "all-tabs-closed", theme);

        let tree = |harness: &mut Ui, name: &str, modifiers: Modifiers| {
            harness
                .get_by_role_and_label(Role::TreeItem, name)
                .click_modifiers(modifiers);
            settle(harness);
        };
        for folder in ["Design", "Journal"] {
            tree(&mut harness, folder, Modifiers::NONE);
        }
        tree(&mut harness, "Website relaunch", Modifiers::COMMAND);
        save(&mut harness, "tabs-one", theme);
        tree(&mut harness, "Type specimens", Modifiers::COMMAND);
        tree(&mut harness, "Brand colors", Modifiers::NONE);
        save(&mut harness, "tabs-preview-and-kept", theme);
        for name in ["Ideas", "Meeting notes", "2026-11-20", "2026-11-21"] {
            tree(&mut harness, name, Modifiers::COMMAND);
        }
        // The tasks open last: with their folder open, the notes under it
        // scroll out of the sidebar.
        tree(&mut harness, "Tasks", Modifiers::NONE);
        for name in [
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

/// Close every tab with its close button.
fn close_tabs(harness: &mut Ui) {
    loop {
        let close = harness
            .query_all_by_role(Role::Button)
            .filter_map(|node| node.accesskit_node().label())
            .find(|label| label.starts_with("Close "));
        let Some(close) = close else {
            break;
        };
        harness.get_by_role_and_label(Role::Button, &close).click();
        settle(harness);
    }
}

/// Calendar's sidebar with Files in every sidebar off, as at first, and
/// on, in light and dark.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn the_calendar_sidebar_renders_with_and_without_files() {
    for theme in [Theme::Light, Theme::Dark] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Studio");
        create_workspace(app_data.path(), &folder);
        write_calendar_workspace(&folder);
        if !install(app_data.path(), parent.path(), &[CALENDAR]) {
            return;
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

/// A PDF tab at Actual Size before and after the interface zooms to 125%
/// from a note, and partway through a pinch over its pages. Prints the
/// page's width on the screen, in pixels, for each.
#[test]
#[ignore = "draws the screens to PNG files, which needs a GPU"]
fn a_pdf_tab_keeps_its_page_size_across_interface_zooms_and_pinches() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Reading");
    create_workspace(app_data.path(), &folder);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app-core/tests/fixtures/pdf");
    std::fs::copy(fixture.join("text.pdf"), folder.join("text.pdf")).unwrap();
    std::fs::write(folder.join("Plan.md"), note('a', "Plan", "To read.\n")).unwrap();
    let app = App::launch_with(app_data.path(), FakeSystem::new(Platform::Linux));
    let mut harness = window(app, Theme::Light, Some(linux_controls()));
    let app = harness.state_mut().app_mut();
    app.open_file("Plan.md", app_core::OpenAs::Kept);
    let note = app.active_tab().unwrap().id();
    app.open_file("text.pdf", app_core::OpenAs::Kept);
    let tab = app.active_tab().unwrap().id();
    let page_pixels = |harness: &Ui| {
        harness
            .get_by_role_and_label(Role::Image, "Page 1 of 3")
            .rect()
            .width()
            * harness.ctx.pixels_per_point()
    };
    let drawn = |harness: &mut Ui| {
        wait_until(harness, |harness| {
            let pdf = harness.state().app().pdf(tab).unwrap();
            harness
                .query_by_role_and_label(Role::Image, "Page 1 of 3")
                .is_some()
                && pdf.settles_in().is_none()
                && pdf.tiles(1).any(|tile| tile.key.density() == pdf.density())
        });
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(30));
            harness.step();
        }
    };
    drawn(&mut harness);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
    drawn(&mut harness);
    save(&mut harness, "9-pdf-actual-size-100", Theme::Light);
    eprintln!("100% interface: page {} pixels wide", page_pixels(&harness));

    harness.state_mut().app_mut().activate_tab(note);
    harness.run_steps(2);
    for _ in 0..2 {
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
        harness.run_steps(2);
    }
    harness.state_mut().app_mut().activate_tab(tab);
    drawn(&mut harness);
    save(&mut harness, "9-pdf-actual-size-125", Theme::Light);
    eprintln!(
        "{}% interface: page {} pixels wide",
        harness.ctx.zoom_factor() * 100.0,
        page_pixels(&harness)
    );

    // Pinch out over the page's first lines.
    let page = harness
        .get_by_role_and_label(Role::Image, "Page 1 of 3")
        .rect();
    harness.hover_at(page.min + egui::vec2(100.0, 60.0));
    harness.run_steps(2);
    for _ in 0..4 {
        harness.event(egui::Event::Zoom(1.1));
        harness.step();
    }
    harness.step();
    save(&mut harness, "9-pdf-pinching", Theme::Light);
    eprintln!("mid-pinch: page {} pixels wide", page_pixels(&harness));
    drawn(&mut harness);
    save(&mut harness, "9-pdf-pinched", Theme::Light);
    eprintln!("settled: page {} pixels wide", page_pixels(&harness));
}

/// The question the website's AI screenshot asks.
const QUESTION: &str = "What's left before the website relaunch?";

/// The answer to [`QUESTION`], from the relaunch note and the studio's
/// tasks. Noura's prompt covers all but its first lines, so the lines
/// under it are short enough to hide behind it whole.
const ANSWER: &str = "Going by **Website relaunch** and your tasks, four things \
stand between you and launch:\n\n\
1. **Send the sitemap to the studio**, four days late\n\
2. **Book the photographer**, due today\n\
3. **Prototype the product grid**, with Theo\n\
4. **Audit the journal links**, by December 2\n\n\
I'll add a task to settle the `/shop` redirects and analytics.";

/// A provider that answers with `text` a few words at a time, then asks
/// to create a task with `task` as its input. Later answers say Done.
struct Answers {
    text: &'static str,
    task: serde_json::Value,
    requests: std::sync::atomic::AtomicUsize,
}

impl Answers {
    fn new(text: &'static str, task: serde_json::Value) -> Self {
        Self {
            text,
            task,
            requests: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl AiStreamer for Answers {
    fn stream(
        &self,
        _request: ResolvedAiStreamRequest,
    ) -> Pin<Box<dyn Future<Output = app_core::ai_client::Result<AiProviderEventStream>> + Send + '_>>
    {
        let first = self
            .requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0;
        Box::pin(async move {
            let text = if first { self.text } else { "Done." };
            let mut events: Vec<_> = text
                .split_inclusive(' ')
                .map(|words| AiProviderStreamEvent::TextDelta(words.to_owned()))
                .collect();
            let tool_calls = if first {
                vec![AiToolCall {
                    call_id: "call_1".into(),
                    name: "tasks__create".into(),
                    arguments: self.task.clone(),
                }]
            } else {
                Vec::new()
            };
            events.push(AiProviderStreamEvent::Completed {
                usage: None,
                stop_reason: None,
                tool_calls,
            });
            let stream: AiProviderEventStream =
                Box::pin(futures::stream::iter(events.into_iter().map(Ok)));
            Ok(stream)
        })
    }
}

/// AppKit's close, minimize and zoom buttons, which it draws over the
/// window, so the renders lack them: each one's color and its edge's.
const WINDOW_BUTTON_COLORS: [([u8; 3], [u8; 3]); 3] = [
    ([0xFF, 0x5F, 0x57], [0xE2, 0x46, 0x3F]),
    ([0xFE, 0xBC, 0x2E], [0xE1, 0xA1, 0x16]),
    ([0x28, 0xC8, 0x40], [0x17, 0xA8, 0x2C]),
];

/// The width of each of AppKit's window buttons, in points, which holds
/// its 12-point circle.
const WINDOW_BUTTON_WIDTH: f32 = 14.0;

/// Paint the window's buttons where the desktop app places them, as
/// circles with a faint edge, smoothed, on `image` drawn at `scale`
/// pixels a point.
fn paint_window_buttons(image: &mut image::RgbaImage, scale: f32) {
    let spot = app_ui::window_buttons(1.0);
    let radius = 6.0 * scale;
    let edge = 0.5 * scale;
    for (index, (fill, rim)) in WINDOW_BUTTON_COLORS.into_iter().enumerate() {
        let offset = (index as f32 - 1.0) * (WINDOW_BUTTON_WIDTH + spot.gap);
        let center = (spot.center + egui::vec2(offset, 0.0)) * scale;
        let first = |middle: f32| (middle - radius - 1.0).floor().max(0.0) as u32;
        let last = |middle: f32| (middle + radius + 1.0).ceil() as u32;
        for y in first(center.y)..last(center.y).min(image.height()) {
            for x in first(center.x)..last(center.x).min(image.width()) {
                let distance = egui::pos2(x as f32 + 0.5, y as f32 + 0.5).distance(center);
                // How much of the pixel the circle covers, and how much of
                // that lies inside the edge.
                let covered = (radius - distance + 0.5).clamp(0.0, 1.0);
                if covered == 0.0 {
                    continue;
                }
                let inside = (radius - edge - distance + 0.5).clamp(0.0, 1.0) / covered;
                let pixel = image.get_pixel_mut(x, y);
                for channel in 0..3 {
                    let color = f32::from(rim[channel])
                        + (f32::from(fill[channel]) - f32::from(rim[channel])) * inside;
                    let under = f32::from(pixel[channel]);
                    pixel[channel] = (under + (color - under) * covered).round() as u8;
                }
            }
        }
    }
}

/// Move the pointer off the window, so nothing shows as hovered.
fn leave(harness: &mut Ui) {
    harness.event(egui::Event::PointerGone);
    settle(harness);
}

/// Save the window as `name`, with the window buttons AppKit would draw.
fn save_mac(harness: &mut Ui, name: &str) {
    leave(harness);
    let mut image = harness.render().expect("the window renders");
    paint_window_buttons(&mut image, harness.ctx.pixels_per_point());
    write_png(&image, name);
}

/// Open the rail's page `page` and wait until the node `role` labeled
/// `ready` shows.
fn open_rail_page(harness: &mut Ui, page: &str, role: Role, ready: &str) {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .get_by_role_and_label(Role::Link, page)
        .click();
    wait_until(harness, |harness| {
        harness.query_by_role_and_label(role, ready).is_some()
    });
}

/// Where the Noura binary sits in a Mac install, which Settings' MCP
/// configuration names.
const MAC_PROGRAM: &str = "/Applications/Noura.app/Contents/MacOS/noura";

/// A macOS window in `theme` on the studio's workspace at `folder`, at
/// 2 pm on Sunday November 22: Tasks, Calendar, Projects and AI on,
/// Dictation and Transcription installed but off, the AI provider
/// `answers`, and the MCP server installed. Keep the folders while the
/// window draws.
fn website_window(
    theme: Theme,
    answers: Answers,
    folder: &Path,
) -> (Ui, (tempfile::TempDir, tempfile::TempDir)) {
    // macOS draws shortcuts such as ⌘K from San Francisco, as the desktop
    // app does.
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
        app_ui::fonts::set_fallback(bytes);
    }
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), folder);
    write_calendar_workspace(folder);
    assert!(
        install(
            app_data.path(),
            parent.path(),
            &[TASKS, CALENDAR, PROJECTS, AI]
        ) && install_turned(
            app_data.path(),
            parent.path(),
            &[DICTATION, TRANSCRIPTION],
            false
        ),
        "build the plugins with crates/plugin-sdk/build-plugins.sh"
    );
    crate::ai::set_up_provider(app_data.path());
    let system = FakeSystem::new(Platform::MacOs);
    system.set_now("2026-11-22T14:00:00Z");
    let mut app = App::launch_with(app_data.path(), system);
    app.set_ai_client(Arc::new(answers));
    let probe = crate::ai_tools::FakeProbe(Ok("noura 0.3.3".into()));
    app.set_mcp_server(Ok(MAC_PROGRAM.into()), Arc::new(probe));
    (window(app, theme, None), (app_data, parent))
}

/// Open the Design, Journal and Reading folders, and keep three notes open
/// in tabs, as in a day's work, with Website relaunch showing. The note's
/// row keeps the keyboard, so the editor shows no caret; focus rings show
/// only after a key press, and none came yet.
fn open_relaunch_note(harness: &mut Ui) {
    for folder in ["Design", "Journal", "Reading"] {
        harness
            .get_by_role_and_label(Role::TreeItem, folder)
            .click();
        settle(harness);
    }
    for note in ["Type specimens", "Meeting notes", "Website relaunch"] {
        harness
            .get_by_role_and_label(Role::TreeItem, note)
            .click_modifiers(Modifiers::COMMAND);
        settle(harness);
    }
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::MultilineTextInput, "Note text")
            .is_some()
    });
    harness
        .get_by_role_and_label(Role::TreeItem, "Website relaunch")
        .focus();
    settle(harness);
}

/// Open the Tasks page on the tasks of the project `project`, which don't
/// repeat its name beside each one.
fn open_project_tasks(harness: &mut Ui, project: &str) {
    open_rail_page(harness, "Tasks", Role::ListBox, "Views");
    harness
        .get_by_role_and_label(Role::ListBox, "Projects")
        .get_by_role_and_label(Role::ListBoxOption, project)
        .click();
    settle(harness);
}

/// Open search, type `query`, and wait until it searched the files'
/// contents.
fn search_for(harness: &mut Ui, query: &str) {
    press(harness, Modifiers::COMMAND, Key::K);
    harness
        .get_by_role_and_label(Role::TextInput, "Search")
        .type_text(query);
    wait_until(harness, |harness| {
        harness.state().app().palette_status() == app_core::PaletteStatus::Results
            && harness.state().app().palette().unwrap().state != app_core::SearchState::Searching
    });
}

/// Send `question` from the AI page's message box with the pointer, as a
/// key would show the focus ring on the tool prompt's Deny, and wait for
/// the prompt.
fn ask(harness: &mut Ui, question: &str) {
    open_rail_page(harness, "AI", Role::Form, MESSAGE);
    let field = || (Role::MultilineTextInput, MESSAGE);
    harness.get_by_role_and_label(field().0, field().1).focus();
    settle(harness);
    harness
        .get_by_role_and_label(field().0, field().1)
        .type_text(question);
    settle(harness);
    harness
        .get_by_role_and_label(Role::Form, MESSAGE)
        .get_by_role_and_label(Role::Button, "Send")
        .click();
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::AlertDialog, TOOL_PROMPT)
            .is_some()
    });
}

/// Noura's prompt before a tool call.
const TOOL_PROMPT: &str = "Allow this tool call?";

/// The AI page's message box.
const MESSAGE: &str = "Message Noura";

/// The screenshots on noura.app, in light, in a macOS window with the
/// buttons AppKit adds: `files`, the relaunch note beside the tree,
/// `tasks`, `calendar`'s month, and `ai`, an answer about the workspace
/// with Noura asking before the tool call that follows it.
/// `scripts/website-screens.sh` turns them into the site's images.
#[test]
#[ignore = "draws the website's screenshots, which needs a GPU"]
fn website_screens() {
    let studio = tempfile::tempdir().unwrap();
    let (mut harness, _folders) = website_window(
        Theme::Light,
        Answers::new(
            ANSWER,
            serde_json::json!({
                "title": "Settle the /shop redirects and analytics",
                "properties": { "priority": "high", "due": "2026-11-23" },
            }),
        ),
        &studio.path().join("Studio"),
    );
    open_relaunch_note(&mut harness);
    save_mac(&mut harness, "files");

    open_project_tasks(&mut harness, "Spring launch");
    harness
        .get_by_role_and_label(Role::ListBoxOption, "Prototype the product grid")
        .click();
    settle(&mut harness);
    save_mac(&mut harness, "tasks");

    open_rail_page(&mut harness, "Calendar", Role::Grid, "Calendar");
    save_mac(&mut harness, "calendar");

    ask(&mut harness, QUESTION);
    save_mac(&mut harness, "ai");
}

/// The question of the website's AI part.
const PART_QUESTION: &str = "Turn the open questions in Website relaunch into tasks.";

/// The answer to [`PART_QUESTION`].
const PART_ANSWER: &str = "**Website relaunch** leaves two questions open:\n\n\
- Do we keep the `/shop` redirects?\n\
- Which analytics, if any\n\n\
I'll add a task for each, due at Monday's design review.";

/// The app's own background around a part of the window, in points.
const PART_MARGIN: f32 = 14.0;

/// Save the part of the window `rect` covers, in points and rounded to
/// whole ones, as `name-light` or `name-dark`, with the pointer off the
/// window.
fn save_component(harness: &mut Ui, name: &str, theme: Theme, rect: egui::Rect) {
    leave(harness);
    let rect = egui::Rect::from_min_max(rect.min.round(), rect.max.round());
    save_part(harness, &format!("{name}-{}", theme_name(theme)), rect);
}

/// The rectangle around every node inside `area` that has one, in points.
fn around_all_in(harness: &Ui, area: egui::Rect) -> egui::Rect {
    let scale = harness.ctx.pixels_per_point();
    harness
        .query_all(egui_kittest::kittest::By::new())
        .filter_map(|node| node.accesskit_node().bounding_box())
        .map(|bounds| {
            egui::Rect::from_min_max(
                egui::pos2(bounds.x0 as f32, bounds.y0 as f32) / scale,
                egui::pos2(bounds.x1 as f32, bounds.y1 as f32) / scale,
            )
        })
        .filter(|rect| area.contains_rect(*rect))
        .fold(egui::Rect::NOTHING, egui::Rect::union)
}

/// The rectangle around the nodes `role` labeled with each of `labels`.
fn around(harness: &Ui, role: Role, labels: &[&str]) -> egui::Rect {
    labels
        .iter()
        .map(|label| harness.get_by_role_and_label(role, label).rect())
        .fold(egui::Rect::NOTHING, egui::Rect::union)
}

/// Where the text `text` shows, in points.
fn text_rect(harness: &Ui, text: &str) -> egui::Rect {
    harness
        .query_all(egui_kittest::kittest::By::new().role(Role::Label))
        .find(|node| node.accesskit_node().value().as_deref() == Some(text))
        .unwrap_or_else(|| panic!("{text} doesn't show"))
        .rect()
}

/// `rect` with [`PART_MARGIN`] above and below it, and as wide as
/// `column`, whose edges draw lines.
fn in_column(rect: egui::Rect, column: egui::Rangef) -> egui::Rect {
    egui::Rect::from_x_y_ranges(column, rect.y_range().expand(PART_MARGIN))
}

/// The studio's workspace at `/Users/Shared/Studio` on a Mac, so the MCP
/// configuration in Settings names a folder as a person's would, or in a
/// temporary folder elsewhere. It refuses a folder that's there already,
/// and removes the one it made.
struct SharedStudio {
    path: PathBuf,
    temporary: Option<tempfile::TempDir>,
}

impl SharedStudio {
    fn new() -> Self {
        let shared = Path::new("/Users/Shared");
        if !shared.is_dir() {
            let temporary = tempfile::tempdir().unwrap();
            return Self {
                path: temporary.path().join("Studio"),
                temporary: Some(temporary),
            };
        }
        let path = shared.join("Studio");
        assert!(
            !path.exists(),
            "{} is there already; move it away to draw the website's parts",
            path.display()
        );
        Self {
            path,
            temporary: None,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SharedStudio {
    fn drop(&mut self) {
        if self.temporary.is_none() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// The line under the MCP configuration in Settings.
const MOVED: &str = "If you move this workspace or Noura, copy the configuration again.";

/// The pieces of the interface the website shows in its feature cards,
/// each cropped from the window with some of the app's background around
/// it, in light and dark: `file-tree`, `task-rows`, `ai-approval`,
/// `plugins`, `mcp`, `calendar-day` and `palette`.
/// `scripts/website-screens.sh` turns them into the site's images.
#[test]
#[ignore = "draws the website's screenshots, which needs a GPU"]
fn website_parts() {
    for theme in [Theme::Light, Theme::Dark] {
        let studio = SharedStudio::new();
        let (mut harness, _folders) = website_window(
            theme,
            Answers::new(
                PART_ANSWER,
                serde_json::json!({
                    "title": "Decide on the /shop redirects",
                    "properties": { "due": "2026-11-23", "project": "[[Spring launch]]" },
                }),
            ),
            studio.path(),
        );
        open_relaunch_note(&mut harness);
        // The sidebar's rows keep 8 points from its edges, and the line
        // under its title row lies 10 above the Files heading.
        let rows = harness.get_by_role_and_label(Role::Tree, "Files").rect();
        let sidebar = rows.x_range().expand(7.5);
        let heading = harness.get_by_role_and_label(Role::Button, "Files").rect();
        let tree = egui::Rect::from_x_y_ranges(
            sidebar,
            heading.top() - 10.0
                ..=around(&harness, Role::TreeItem, &["Meeting notes"]).bottom() + PART_MARGIN,
        );
        save_component(&mut harness, "file-tree", theme, tree);

        // The palette over the empty window, before any chat, with its
        // caret on as it shows between blinks.
        close_tabs(&mut harness);
        harness
            .ctx
            .all_styles_mut(|style| style.visuals.text_cursor.blink = false);
        search_for(&mut harness, "journal");
        let field = harness
            .get_by_role_and_label(Role::TextInput, "Search")
            .rect();
        let found = harness
            .query_all_by_role(Role::ListBoxOption)
            .map(|result| result.rect())
            .filter(|result| field.x_range().contains(result.center().x))
            .fold(field, egui::Rect::union);
        // The palette's frame holds its field and results 8 points in, and
        // 16 below the last.
        let palette = egui::Rect::from_min_max(
            found.min - egui::vec2(8.0, 8.0),
            found.max + egui::vec2(8.0, 16.0),
        );
        save_component(&mut harness, "palette", theme, palette.expand(PART_MARGIN));
        press(&mut harness, Modifiers::NONE, Key::Escape);
        harness
            .ctx
            .all_styles_mut(|style| style.visuals.text_cursor.blink = true);

        open_project_tasks(&mut harness, "Studio admin");
        let list = harness
            .get_by_role_and_label(Role::ListBoxOption, "Order sample prints")
            .rect();
        let rows = text_rect(&harness, "Upcoming").union(list);
        save_component(
            &mut harness,
            "task-rows",
            theme,
            in_column(rows, list.x_range().expand(7.5)),
        );

        ask(&mut harness, PART_QUESTION);
        let message = harness
            .get_by_role_and_label(Role::MultilineTextInput, MESSAGE)
            .rect();
        // The thread between the Orb's name and the message box, with the
        // 24 points the prompt's frame keeps around its text.
        let thread = egui::Rect::from_x_y_ranges(
            message.left() - 40.0..=harness.ctx.viewport_rect().right(),
            100.0..=message.top() - 20.0,
        );
        let conversation = around_all_in(&harness, thread).expand(24.0 + PART_MARGIN);
        save_component(&mut harness, "ai-approval", theme, conversation);
        harness.get_by_role_and_label(Role::Button, "Deny").click();
        settle(&mut harness);

        harness
            .get_by_role_and_label(Role::Button, "Settings")
            .click();
        settle(&mut harness);
        harness
            .get_by_role_and_label(Role::Navigation, "Settings sections")
            .get_by_role_and_label(Role::Button, "Plugins")
            .click();
        settle(&mut harness);
        let plugins = text_rect(&harness, "Plugins")
            .union(around(&harness, Role::Button, &["Transcription settings"]))
            .union(around(&harness, Role::Switch, &["Enable Transcription"]));
        save_component(&mut harness, "plugins", theme, plugins.expand(PART_MARGIN));
        harness
            .get_by_role_and_label(Role::Navigation, "Settings sections")
            .get_by_role_and_label(Role::Button, "External tools")
            .click();
        settle(&mut harness);
        // From the warning's frame, 12 points around its text, to the line
        // under the configuration.
        let warning = text_rect(&harness, "Assistants edit files directly");
        let mcp = egui::Rect::from_min_max(
            egui::pos2(
                text_rect(&harness, "Configuration").left(),
                warning.top() - 12.0,
            ),
            egui::pos2(
                harness
                    .get_by_role_and_label(Role::Button, "Copy")
                    .rect()
                    .right(),
                text_rect(&harness, MOVED).bottom(),
            ),
        );
        save_component(&mut harness, "mcp", theme, mcp.expand(PART_MARGIN));
        press(&mut harness, Modifiers::NONE, Key::Escape);

        open_rail_page(&mut harness, "Calendar", Role::Grid, "Calendar");
        harness
            .get_by_role_and_label(Role::RadioButton, "Day")
            .click();
        settle(&mut harness);
        harness
            .get_by_role_and_label(Role::Button, "Next day")
            .click();
        settle(&mut harness);
        // A narrower window, so the day's column reads at a card's width:
        // from its date to the 2 PM label, which sits on its hour's line,
        // between the sidebar and the window's edge.
        harness.set_size(egui::vec2(860.0, 780.0));
        settle(&mut harness);
        let day = egui::Rect::from_x_y_ranges(
            harness
                .get_by_role_and_label(Role::Splitter, "Resize panel")
                .rect()
                .right()..=harness.ctx.viewport_rect().right(),
            harness
                .get_by_role_and_label(Role::ColumnHeader, "Monday, November 23, 2026")
                .rect()
                .top()
                ..=harness
                    .get_by_role_and_label(Role::RowHeader, "2 PM")
                    .rect()
                    .top()
                    + 12.0,
        );
        save_component(&mut harness, "calendar-day", theme, day);
    }
}
