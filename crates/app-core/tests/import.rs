//! Seam 1: the files service's import, which dragging files in from the
//! file manager uses.

mod support;

use support::Fixture;

#[test]
fn import_copies_outside_files_and_folders_into_a_folder() {
    let fixture = Fixture::new(&[("Docs/Plan.md", b"p")]);
    let outside = fixture.outside();
    std::fs::write(outside.join("scan.pdf"), b"%PDF").unwrap();
    std::fs::create_dir(outside.join("Photos")).unwrap();
    std::fs::write(outside.join("Photos/cat.jpg"), b"jpg").unwrap();
    let app = fixture.app();
    let workspace = app.workspace().unwrap();

    let imported = workspace
        .import(&[outside.join("scan.pdf"), outside.join("Photos")], "Docs")
        .unwrap();

    assert_eq!(imported.paths, ["Docs/scan.pdf", "Docs/Photos"]);
    assert_eq!(imported.skipped, 0);
    assert_eq!(fixture.read("Docs/scan.pdf"), b"%PDF");
    assert_eq!(fixture.read("Docs/Photos/cat.jpg"), b"jpg");
    assert_eq!(std::fs::read(outside.join("scan.pdf")).unwrap(), b"%PDF");
}

#[test]
fn import_refuses_a_folder_into_itself_and_copies_nothing() {
    let fixture = Fixture::new(&[("Docs/Deep/Plan.md", b"p")]);
    let outside = fixture.outside();
    std::fs::write(outside.join("first.txt"), b"1").unwrap();
    let app = fixture.app();
    let workspace = app.workspace().unwrap();
    let before = fixture.snapshot();

    for folder in ["Docs", "Docs/Deep"] {
        let error = workspace
            .import(
                &[outside.join("first.txt"), fixture.root.join("Docs")],
                folder,
            )
            .unwrap_err();
        assert_eq!(error.message, "Noura can't copy a folder into itself.");
    }

    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn import_refuses_a_folder_that_holds_the_workspace_and_copies_nothing() {
    let fixture = Fixture::new(&[("Readme.md", b"r")]);
    let outside = fixture.outside();
    std::fs::write(outside.join("first.txt"), b"1").unwrap();
    let app = fixture.app();
    let workspace = app.workspace().unwrap();
    let before = fixture.snapshot();
    let holder = fixture.root.parent().unwrap().to_owned();

    for sources in [
        vec![outside.join("first.txt"), holder.clone()],
        vec![fixture.root.clone()],
    ] {
        let error = workspace.import(&sources, "").unwrap_err();
        assert_eq!(
            error.message,
            "That folder holds this workspace, so Noura can't copy it here."
        );
    }

    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn import_refuses_a_destination_outside_the_workspace() {
    let fixture = Fixture::new(&[("Readme.md", b"r")]);
    let outside = fixture.outside();
    std::fs::write(outside.join("first.txt"), b"1").unwrap();
    let app = fixture.app();
    let workspace = app.workspace().unwrap();

    for folder in ["..", "../Outside", "/tmp", ".noura", "Readme.md"] {
        assert!(
            workspace
                .import(&[outside.join("first.txt")], folder)
                .is_err(),
            "{folder}"
        );
    }
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
}
