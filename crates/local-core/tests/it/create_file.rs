//! `create_file` writes new bytes durably, lists them, and refuses a taken
//! path or one that leaves the workspace.

use local_core::WorkspaceEngine;

#[test]
fn a_new_file_lands_once_and_never_outside_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    let app_data = tempfile::tempdir().unwrap();
    let engine = WorkspaceEngine::create_with_app_data(root.path(), app_data.path()).unwrap();
    let wav = b"RIFF....WAVEfmt ".to_vec();

    engine
        .create_file("Meetings/Standup/Standup.wav", &wav)
        .unwrap();

    assert_eq!(
        std::fs::read(root.path().join("Meetings/Standup/Standup.wav")).unwrap(),
        wav
    );
    assert!(
        engine
            .list_workspace_entries()
            .unwrap()
            .iter()
            .any(|entry| entry.relative_path == "Meetings/Standup/Standup.wav")
    );
    let taken = engine
        .create_file("Meetings/Standup/Standup.wav", b"other")
        .unwrap_err();
    assert_eq!(taken.code, "path_exists");
    assert_eq!(
        std::fs::read(root.path().join("Meetings/Standup/Standup.wav")).unwrap(),
        wav
    );
    assert!(engine.create_file("../escape.wav", b"x").is_err());
    assert!(engine.create_file(".noura/x.wav", b"x").is_err());
    assert!(!root.path().parent().unwrap().join("escape.wav").exists());
}
