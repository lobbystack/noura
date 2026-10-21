//! Seam 1: showing items in the file manager, copying paths and opening a
//! terminal, with a fake system opener and clipboard.

mod support;

use app_core::Platform;
use support::{Call, FakeSystem, Fixture, notice};

fn fixture() -> Fixture {
    Fixture::new(&[("Docs/Plan.md", b"p"), ("Top.md", b"t")])
}

#[test]
fn macos_and_windows_select_the_item_in_the_file_manager() {
    for platform in [Platform::MacOs, Platform::Windows] {
        let fixture = fixture();
        let system = FakeSystem::on(platform);
        let mut app = fixture.app_with(system.clone());

        app.reveal("Docs/Plan.md");
        app.reveal("Docs");
        app.reveal_workspace();

        assert_eq!(
            system.calls(),
            [
                Call::Reveal(fixture.root.join("Docs").join("Plan.md")),
                Call::Reveal(fixture.root.join("Docs")),
                Call::Reveal(fixture.root.clone()),
            ],
            "{platform:?}"
        );
        assert!(app.notice().is_none());
    }
}

#[test]
fn linux_opens_the_folder_that_holds_the_item() {
    let fixture = fixture();
    let system = FakeSystem::on(Platform::Linux);
    let mut app = fixture.app_with(system.clone());

    app.reveal("Docs/Plan.md");
    app.reveal("Top.md");
    app.reveal("Docs");
    app.reveal_workspace();

    assert_eq!(
        system.calls(),
        [
            Call::OpenFolder(fixture.root.join("Docs")),
            Call::OpenFolder(fixture.root.clone()),
            Call::OpenFolder(fixture.root.clone()),
            Call::OpenFolder(fixture.root.parent().unwrap().to_owned()),
        ]
    );
}

#[test]
fn the_menu_names_the_file_manager_of_each_platform() {
    let fixture = fixture();
    for (platform, label) in [
        (Platform::MacOs, "Reveal in Finder"),
        (Platform::Windows, "Show in Explorer"),
        (Platform::Linux, "Show in file manager"),
    ] {
        let app = fixture.app_with(FakeSystem::on(platform));
        assert_eq!(app.platform().reveal_label(), label);
    }
}

#[test]
fn copy_path_puts_the_workspace_relative_path_on_the_clipboard() {
    let fixture = fixture();
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());

    app.copy_path("Docs/Plan.md");
    app.copy_path("Docs");
    app.copy_path("Top.md");

    assert_eq!(
        system.calls(),
        [
            Call::Copy("Docs/Plan.md".to_owned()),
            Call::Copy("Docs".to_owned()),
            Call::Copy("Top.md".to_owned()),
        ]
    );
}

#[test]
fn open_in_terminal_opens_a_folder_or_a_files_folder_on_macos_and_windows() {
    for platform in [Platform::MacOs, Platform::Windows] {
        let fixture = fixture();
        let system = FakeSystem::on(platform);
        let mut app = fixture.app_with(system.clone());
        assert!(app.can_open_terminal());

        app.open_terminal("Docs/Plan.md");
        app.open_terminal("Docs");
        app.open_terminal("Top.md");

        assert_eq!(
            system.calls(),
            [
                Call::OpenTerminal(fixture.root.join("Docs")),
                Call::OpenTerminal(fixture.root.join("Docs")),
                Call::OpenTerminal(fixture.root.clone()),
            ],
            "{platform:?}"
        );
    }
}

#[test]
fn linux_has_no_terminal_noura_can_rely_on() {
    let fixture = fixture();
    let system = FakeSystem::on(Platform::Linux);
    let mut app = fixture.app_with(system.clone());

    assert!(!app.can_open_terminal());
    app.open_terminal("Docs");

    assert!(system.calls().is_empty());
}

#[test]
fn nothing_outside_the_workspace_reaches_the_system() {
    let fixture = fixture();
    let outside = fixture.outside();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, fixture.root.join("Linked")).unwrap();
    let system = FakeSystem::on(Platform::MacOs);
    let mut app = fixture.app_with(system.clone());

    for path in ["", "..", "../Outside", "/tmp", "Linked", "Missing.md"] {
        app.reveal(path);
        assert!(app.notice().is_some(), "{path}");
        app.copy_path(path);
        app.open_terminal(path);
    }

    assert!(system.calls().is_empty(), "{:?}", system.calls());
    app.reveal("Missing.md");
    assert_eq!(
        notice(&app),
        Some((
            "Couldn't show \u{201c}Missing\u{201d}".to_owned(),
            "Missing.md isn't in the workspace anymore.".to_owned()
        ))
    );
}

#[test]
fn a_file_that_is_not_downloaded_can_be_shown_and_its_path_copied() {
    // iCloud keeps a placeholder in place of a file it hasn't downloaded.
    let fixture = Fixture::new(&[
        ("Docs/Plan.md", b"p"),
        ("Docs/.Scan.pdf.icloud", b"bplist00"),
    ]);
    let system = FakeSystem::on(Platform::MacOs);
    let mut app = fixture.app_with(system.clone());

    app.reveal("Docs/Scan.pdf");
    app.copy_path("Docs/Scan.pdf");
    app.open_terminal("Docs/Scan.pdf");

    assert_eq!(
        system.calls(),
        [
            Call::Reveal(fixture.root.join("Docs").join(".Scan.pdf.icloud")),
            Call::Copy("Docs/Scan.pdf".to_owned()),
            Call::OpenTerminal(fixture.root.join("Docs")),
        ]
    );
    assert!(app.notice().is_none(), "{:?}", notice(&app));
    assert_eq!(fixture.read("Docs/.Scan.pdf.icloud"), b"bplist00");
}
