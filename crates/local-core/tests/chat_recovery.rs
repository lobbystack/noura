//! Chat recovery: what it decides from, and what a chat mutation that can't
//! finish does to the rest of the workspace. Each test sets up the exact
//! moment through a recovery hook, so none of them depends on timing.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use local_core::{
    AppendChatToolResultInput, AppendChatUserMessageInput, BeginChatToolCallInput, Chat,
    ChatMessage, ChatMessageStatus, ChatRecoveryPoint, ChatRetention, CreateChatInput,
    CreateObjectInput, MutationResult, ObjectPatch, WatchFault, WorkspaceEngine,
};
use tempfile::{TempDir, tempdir};

/// An engine no watcher report reaches, so only the code under test marks
/// the index stale.
fn quiet_engine() -> (TempDir, TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Recovery", app_data.path())
            .unwrap();
    engine.force_watch_fault(WatchFault::Silent);
    (workspace, app_data, engine)
}

fn create_chat(engine: &WorkspaceEngine, title: &str) -> MutationResult<Chat> {
    engine
        .create_chat(CreateChatInput {
            title: title.into(),
            retention: None,
            retention_days: None,
        })
        .unwrap()
}

/// A chat whose run died with one tool call open.
fn chat_with_open_call(engine: &WorkspaceEngine) -> (String, MutationResult<ChatMessage>) {
    let chat = create_chat(engine, "Open call");
    let call = engine
        .begin_chat_tool_call(BeginChatToolCallInput {
            chat_id: chat.value.id.clone(),
            run_id: "run_recovery".into(),
            tool_call_id: "call_recovery".into(),
            tool_name: "workspace.search".into(),
            content: r#"{"query":"status"}"#.into(),
            expected_chat_revision: chat.revision,
        })
        .unwrap();
    (chat.value.id, call)
}

fn append_result(engine: &WorkspaceEngine, chat_id: &str) -> MutationResult<ChatMessage> {
    let revision = engine.read_chat(chat_id).unwrap().chat.revision;
    engine
        .append_chat_tool_result(AppendChatToolResultInput {
            chat_id: chat_id.to_owned(),
            run_id: "run_recovery".into(),
            tool_call_id: "call_recovery".into(),
            tool_name: "workspace.search".into(),
            content: r#"{"matches":1}"#.into(),
            expected_chat_revision: revision,
        })
        .unwrap()
}

#[expect(
    clippy::result_large_err,
    reason = "Tests pass CoreError through as the engine returns it"
)]
fn append_user(
    engine: &WorkspaceEngine,
    chat_id: &str,
    revision: String,
) -> local_core::Result<MutationResult<ChatMessage>> {
    engine.append_chat_user_message(AppendChatUserMessageInput {
        chat_id: chat_id.to_owned(),
        run_id: "run_user".into(),
        content: "hello".into(),
        expected_chat_revision: revision,
    })
}

fn status_of(engine: &WorkspaceEngine, chat_id: &str, message_id: &str) -> ChatMessageStatus {
    engine
        .read_chat(chat_id)
        .unwrap()
        .messages
        .into_iter()
        .find(|message| message.id == message_id)
        .unwrap()
        .status
}

fn intents(workspace: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(workspace.join(".noura/chat-mutations")) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|value| value == "json"))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn set_aside(workspace: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(workspace.join(".noura/chat-mutations")) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|value| value == "unapplied"))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// A chat whose user message reached its file and whose chat file a failed
/// write left behind: the state a crash leaves between the two writes.
fn chat_cut_off_before_its_chat_file(
    workspace: &Path,
    engine: &WorkspaceEngine,
) -> (MutationResult<Chat>, PathBuf) {
    let chat = create_chat(engine, "Cut off");
    engine.fail_chat_mutation_write_for_testing("chat", 1);
    let error = append_user(engine, &chat.value.id, chat.revision.clone()).unwrap_err();
    assert_eq!(error.code, "chat_mutation_write_failed");
    assert_eq!(intents(workspace).len(), 1);
    let path = workspace.join(&chat.value.relative_path);
    (chat, path)
}

fn edit_title_outside(path: &Path, from: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    let edited = text.replace(&format!("title: {from}"), "title: Edited outside Noura");
    assert_ne!(edited, text);
    std::fs::write(path, edited).unwrap();
}

/// The rest of the workspace reads and writes, and so does another chat.
fn assert_workspace_still_works(engine: &WorkspaceEngine) {
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Still works".into(),
            body: "body".into(),
            relative_path: None,
            properties: Default::default(),
        })
        .unwrap();
    let patch = ObjectPatch {
        title: None,
        body: Some("changed".into()),
        properties: Default::default(),
        remove_properties: Vec::new(),
        expected_revision: note.revision,
    };
    engine.update_object(&note.value.id, patch).unwrap();
    assert!(!engine.list_objects("note").unwrap().is_empty());
    let other = create_chat(engine, "Another chat");
    append_user(engine, &other.value.id, other.revision).unwrap();
    assert_eq!(engine.read_chat(&other.value.id).unwrap().messages.len(), 1);
    assert!(
        engine
            .list_chats()
            .unwrap()
            .iter()
            .any(|chat| chat.id == other.value.id)
    );
    engine.reconcile().unwrap();
}

#[test]
fn recovery_counts_a_tool_result_that_lands_after_its_walk() {
    let (_workspace, _app_data, engine) = quiet_engine();
    let (chat_id, call) = chat_with_open_call(&engine);
    let hook_chat = chat_id.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::BeforeWriteLock, move |engine| {
        append_result(engine, &hook_chat);
    });

    let recovered = engine.recover_interrupted_chat(&chat_id).unwrap();

    let call = recovered
        .messages
        .iter()
        .find(|message| message.id == call.value.id)
        .unwrap();
    assert_eq!(call.status, ChatMessageStatus::Completed);
    assert_eq!(recovered.messages.len(), 2);
}

#[test]
fn recovery_counts_a_result_file_that_lands_after_its_walk_and_past_the_index() {
    let (workspace, _app_data, engine) = quiet_engine();
    let (chat_id, call) = chat_with_open_call(&engine);
    let result = append_result(&engine, &chat_id);
    let result_path = workspace.path().join(&result.value.relative_path);
    let result_bytes = std::fs::read(&result_path).unwrap();
    std::fs::remove_file(&result_path).unwrap();
    engine.reconcile().unwrap();
    engine.on_chat_recovery_point(ChatRecoveryPoint::BeforeWriteLock, move |_| {
        // A sync or another app: nothing tells the index.
        std::fs::write(&result_path, result_bytes).unwrap();
    });

    let recovered = engine.recover_interrupted_chat(&chat_id).unwrap();

    assert_eq!(
        recovered
            .messages
            .iter()
            .find(|message| message.id == call.value.id)
            .unwrap()
            .status,
        ChatMessageStatus::Completed
    );
}

#[test]
fn recovery_of_a_stale_chat_revision_writes_nothing() {
    let (workspace, _app_data, engine) = quiet_engine();
    let (chat_id, call) = chat_with_open_call(&engine);
    let chat_path = workspace
        .path()
        .join(&engine.read_chat(&chat_id).unwrap().chat.relative_path);
    let call_path = workspace.path().join(&call.value.relative_path);
    let call_bytes = std::fs::read(&call_path).unwrap();
    let hook_path = chat_path.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::AfterDecidingRead, move |_| {
        edit_title_outside(&hook_path, "Open call");
    });

    let error = engine.recover_interrupted_chat(&chat_id).unwrap_err();

    assert_eq!(error.code, "revision_conflict");
    assert!(intents(workspace.path()).is_empty());
    assert_eq!(std::fs::read(&call_path).unwrap(), call_bytes);
    assert!(
        std::fs::read_to_string(&chat_path)
            .unwrap()
            .contains("title: Edited outside Noura")
    );

    // Nothing was left to replay, so the next try settles the call on top
    // of the edit.
    engine.reconcile().unwrap();
    assert_eq!(
        status_of(&engine, &chat_id, &call.value.id),
        ChatMessageStatus::InProgress
    );
    engine.age_failed_chat_recovery_for_testing(&chat_id, Duration::from_secs(60));
    engine.recover_interrupted_chat(&chat_id).unwrap();
    assert_eq!(
        status_of(&engine, &chat_id, &call.value.id),
        ChatMessageStatus::Interrupted
    );
    assert!(
        std::fs::read_to_string(&chat_path)
            .unwrap()
            .contains("title: Edited outside Noura")
    );
}

/// The message file an intent on disk names.
fn message_named_by_intent(workspace: &Path) -> PathBuf {
    let intent = std::fs::read(&intents(workspace)[0]).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&intent).unwrap();
    workspace.join(parsed["message"]["relative_path"].as_str().unwrap())
}

fn files_under(folder: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return files;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn an_edit_after_the_intent_reaches_disk_fails_the_write_and_leaves_the_chat_readable() {
    let (workspace, _app_data, engine) = quiet_engine();
    let chat = create_chat(&engine, "Edited");
    let chat_path = workspace.path().join(&chat.value.relative_path);
    let hook_path = chat_path.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::IntentWritten, move |_| {
        edit_title_outside(&hook_path, "Edited");
    });
    let wrote_message = Arc::new(AtomicBool::new(false));
    let hook_wrote = wrote_message.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::MessageWritten, move |_| {
        hook_wrote.store(true, Ordering::SeqCst);
    });

    let error = append_user(&engine, &chat.value.id, chat.revision.clone()).unwrap_err();

    assert_eq!(error.code, "revision_conflict");
    assert!(
        !wrote_message.load(Ordering::SeqCst),
        "the check after the intent stops the commit before it writes"
    );
    assert!(intents(workspace.path()).is_empty());
    assert!(engine.index_marked_stale_for_testing());
    assert!(files_under(&chat_path.parent().unwrap().join("messages")).is_empty());
    let read = engine.read_chat(&chat.value.id).unwrap();
    assert!(read.messages.is_empty());
    assert_eq!(read.chat.title, "Edited outside Noura");
    append_user(&engine, &chat.value.id, read.chat.revision).unwrap();
    assert_eq!(engine.read_chat(&chat.value.id).unwrap().messages.len(), 1);
}

#[test]
fn a_recovery_an_edit_beats_after_its_intent_can_try_again() {
    let (workspace, _app_data, engine) = quiet_engine();
    let (chat_id, call) = chat_with_open_call(&engine);
    let chat_path = workspace
        .path()
        .join(&engine.read_chat(&chat_id).unwrap().chat.relative_path);
    let call_path = workspace.path().join(&call.value.relative_path);
    let call_bytes = std::fs::read(&call_path).unwrap();
    let hook_path = chat_path.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::IntentWritten, move |_| {
        edit_title_outside(&hook_path, "Open call");
    });

    let error = engine.recover_interrupted_chat(&chat_id).unwrap_err();

    assert_eq!(error.code, "revision_conflict");
    assert!(intents(workspace.path()).is_empty());
    assert_eq!(std::fs::read(&call_path).unwrap(), call_bytes);
    engine.age_failed_chat_recovery_for_testing(&chat_id, Duration::from_secs(60));
    engine.recover_interrupted_chat(&chat_id).unwrap();
    assert_eq!(
        status_of(&engine, &chat_id, &call.value.id),
        ChatMessageStatus::Interrupted
    );
}

#[test]
fn a_new_message_goes_when_an_edit_beats_the_chat_file() {
    let (workspace, _app_data, engine) = quiet_engine();
    let chat = create_chat(&engine, "Edited");
    let chat_path = workspace.path().join(&chat.value.relative_path);
    let hook_path = chat_path.clone();
    let messages = chat_path.parent().unwrap().join("messages");
    let hook_messages = messages.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::MessageWritten, move |_| {
        assert_eq!(files_under(&hook_messages).len(), 1);
        edit_title_outside(&hook_path, "Edited");
    });

    let error = append_user(&engine, &chat.value.id, chat.revision.clone()).unwrap_err();

    assert_eq!(error.code, "revision_conflict");
    assert!(intents(workspace.path()).is_empty());
    assert!(files_under(&messages).is_empty());
    assert!(
        engine
            .read_chat(&chat.value.id)
            .unwrap()
            .messages
            .is_empty()
    );
}

#[test]
fn a_finished_message_goes_back_when_an_edit_beats_the_chat_file() {
    let (workspace, _app_data, engine) = quiet_engine();
    let (chat_id, call) = chat_with_open_call(&engine);
    let chat_path = workspace
        .path()
        .join(&engine.read_chat(&chat_id).unwrap().chat.relative_path);
    let call_path = workspace.path().join(&call.value.relative_path);
    let call_bytes = std::fs::read(&call_path).unwrap();
    let hook_call = call_path.clone();
    let hook_bytes = call_bytes.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::MessageWritten, move |_| {
        assert_ne!(std::fs::read(&hook_call).unwrap(), hook_bytes);
        edit_title_outside(&chat_path, "Open call");
    });

    let error = engine.recover_interrupted_chat(&chat_id).unwrap_err();

    assert_eq!(error.code, "revision_conflict");
    assert!(intents(workspace.path()).is_empty());
    assert_eq!(std::fs::read(&call_path).unwrap(), call_bytes);
    assert_eq!(
        status_of(&engine, &chat_id, &call.value.id),
        ChatMessageStatus::InProgress
    );
}

#[test]
fn undoing_a_commit_keeps_a_message_file_that_changed_after_it() {
    let (workspace, _app_data, engine) = quiet_engine();
    let (chat_id, call) = chat_with_open_call(&engine);
    let chat_path = workspace
        .path()
        .join(&engine.read_chat(&chat_id).unwrap().chat.relative_path);
    let call_path = workspace.path().join(&call.value.relative_path);
    let hook_call = call_path.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::MessageWritten, move |_| {
        let text = std::fs::read_to_string(&hook_call).unwrap();
        std::fs::write(&hook_call, format!("{text}\nEdited outside Noura\n")).unwrap();
        edit_title_outside(&chat_path, "Open call");
    });

    let error = engine.recover_interrupted_chat(&chat_id).unwrap_err();

    assert_eq!(error.code, "revision_conflict");
    assert!(intents(workspace.path()).is_empty());
    assert!(
        std::fs::read_to_string(&call_path)
            .unwrap()
            .contains("Edited outside Noura")
    );
}

#[test]
fn undoing_a_commit_keeps_a_new_message_file_that_changed_after_it() {
    let (workspace, _app_data, engine) = quiet_engine();
    let chat = create_chat(&engine, "Edited");
    let chat_path = workspace.path().join(&chat.value.relative_path);
    let messages = chat_path.parent().unwrap().join("messages");
    let hook_messages = messages.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::MessageWritten, move |_| {
        let path = files_under(&hook_messages).remove(0);
        let edited = std::fs::read_to_string(&path).unwrap() + "\nEdited outside Noura\n";
        std::fs::write(path, edited).unwrap();
        edit_title_outside(&chat_path, "Edited");
    });

    let error = append_user(&engine, &chat.value.id, chat.revision.clone()).unwrap_err();

    assert_eq!(error.code, "revision_conflict");
    assert!(intents(workspace.path()).is_empty());
    assert_eq!(files_under(&messages).len(), 1);
}

#[test]
fn a_crash_plus_an_edit_to_the_chat_file_keeps_the_edit_and_the_message() {
    let (workspace, app_data, engine) = quiet_engine();
    let (chat, chat_path) = chat_cut_off_before_its_chat_file(workspace.path(), &engine);
    edit_title_outside(&chat_path, "Cut off");
    let edited = std::fs::read(&chat_path).unwrap();
    let index_path = engine.index_path().to_owned();

    // The crash, with the derived index gone too.
    drop(engine);
    std::fs::remove_file(&index_path).unwrap();
    let engine = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();

    let read = engine.read_chat(&chat.value.id).unwrap();
    assert_eq!(read.messages.len(), 1);
    assert_eq!(read.messages[0].content, "hello");
    assert_eq!(read.chat.title, "Edited outside Noura");
    assert_eq!(std::fs::read(&chat_path).unwrap(), edited);
    assert!(intents(workspace.path()).is_empty());
    assert!(set_aside(workspace.path()).is_empty());
    assert_workspace_still_works(&engine);
    engine.rebuild_index().unwrap();
    append_user(&engine, &chat.value.id, read.chat.revision).unwrap();
}

#[test]
fn replay_writes_a_message_the_crash_kept_from_its_file_past_an_edited_chat_file() {
    let (workspace, _app_data, engine) = quiet_engine();
    let chat = create_chat(&engine, "Cut off");
    engine.fail_chat_mutation_write_for_testing("message", 1);
    let error = append_user(&engine, &chat.value.id, chat.revision.clone()).unwrap_err();
    assert_eq!(error.code, "chat_mutation_write_failed");
    let message = message_named_by_intent(workspace.path());
    assert!(!message.exists());
    let chat_path = workspace.path().join(&chat.value.relative_path);
    edit_title_outside(&chat_path, "Cut off");
    let edited = std::fs::read(&chat_path).unwrap();

    engine.reconcile().unwrap();

    assert!(message.exists());
    assert_eq!(std::fs::read(&chat_path).unwrap(), edited);
    assert!(intents(workspace.path()).is_empty());
    assert_eq!(engine.read_chat(&chat.value.id).unwrap().messages.len(), 1);
}

#[test]
fn a_crash_plus_an_edit_to_the_message_file_keeps_the_edit_and_sets_the_intent_aside() {
    let (workspace, _app_data, engine) = quiet_engine();
    let (chat_id, call) = chat_with_open_call(&engine);
    let read = engine.read_chat(&chat_id).unwrap();
    let chat_path = workspace.path().join(&read.chat.relative_path);
    let chat_bytes = std::fs::read(&chat_path).unwrap();
    engine.fail_chat_mutation_write_for_testing("message", 1);
    let error = engine.recover_interrupted_chat(&chat_id).unwrap_err();
    assert_eq!(error.code, "chat_mutation_write_failed");
    let call_path = workspace.path().join(&call.value.relative_path);
    let edited = std::fs::read_to_string(&call_path)
        .unwrap()
        .replace("\"query\"", "\"edited\"");
    std::fs::write(&call_path, &edited).unwrap();

    engine.reconcile().unwrap();

    assert_eq!(std::fs::read_to_string(&call_path).unwrap(), edited);
    assert_eq!(std::fs::read(&chat_path).unwrap(), chat_bytes);
    assert!(intents(workspace.path()).is_empty());
    assert_eq!(set_aside(workspace.path()).len(), 1);
    assert_eq!(
        status_of(&engine, &chat_id, &call.value.id),
        ChatMessageStatus::InProgress
    );
}

#[test]
fn a_crash_plus_a_chat_file_that_left_writes_nothing_and_sets_the_intent_aside() {
    let (workspace, _app_data, engine) = quiet_engine();
    let chat = create_chat(&engine, "Moved");
    engine.fail_chat_mutation_write_for_testing("message", 1);
    append_user(&engine, &chat.value.id, chat.revision.clone()).unwrap_err();
    let message = message_named_by_intent(workspace.path());
    let chat_path = workspace.path().join(&chat.value.relative_path);
    let folder = chat_path.parent().unwrap();
    let moved = folder.with_file_name("moved-chat");
    std::fs::rename(folder, &moved).unwrap();

    engine.reconcile().unwrap();

    assert!(!message.exists());
    assert!(!folder.exists());
    assert!(intents(workspace.path()).is_empty());
    assert_eq!(set_aside(workspace.path()).len(), 1);
    let read = engine.read_chat(&chat.value.id).unwrap();
    assert!(read.messages.is_empty());
    append_user(&engine, &chat.value.id, read.chat.revision).unwrap();
}

#[cfg(unix)]
#[test]
fn a_target_the_disk_refuses_to_read_keeps_its_intent() {
    use std::os::unix::fs::PermissionsExt;

    let (workspace, _app_data, engine) = quiet_engine();
    let chat = create_chat(&engine, "Refused");
    engine.fail_chat_mutation_write_for_testing("message", 1);
    append_user(&engine, &chat.value.id, chat.revision.clone()).unwrap_err();
    let chat_path = workspace.path().join(&chat.value.relative_path);
    std::fs::set_permissions(&chat_path, std::fs::Permissions::from_mode(0o000)).unwrap();

    engine.reconcile().ok();

    std::fs::set_permissions(&chat_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(intents(workspace.path()).len(), 1);
    assert!(set_aside(workspace.path()).is_empty());
    assert_eq!(engine.read_chat(&chat.value.id).unwrap().messages.len(), 1);
    assert!(intents(workspace.path()).is_empty());
}

#[test]
fn broken_intents_stay_on_disk_and_stop_at_the_chat_they_name() {
    let (workspace, app_data, engine) = quiet_engine();
    let chat = create_chat(&engine, "Named by a bad intent");
    let directory = workspace.path().join(".noura/chat-mutations");
    std::fs::create_dir_all(&directory).unwrap();
    let garbage = directory.join("garbage.json");
    std::fs::write(&garbage, b"{ not json").unwrap();
    let escaping = directory.join("escaping.json");
    std::fs::write(
        &escaping,
        serde_json::json!({
            "version": 1,
            "chat": { "relative_path": "../outside.md", "expected_revision": null, "bytes": "x" },
            "message": { "relative_path": "../outside.md", "expected_revision": null, "bytes": "x" },
        })
        .to_string(),
    )
    .unwrap();
    let future = directory.join("future.json");
    std::fs::write(
        &future,
        serde_json::json!({
            "version": 2,
            "chat": { "relative_path": chat.value.relative_path, "expected_revision": null, "bytes": "x" },
            "message": { "relative_path": "chats/a/messages/b.md", "expected_revision": null, "bytes": "x" },
        })
        .to_string(),
    )
    .unwrap();

    assert_workspace_still_works(&engine);
    let error = engine.read_chat(&chat.value.id).unwrap_err();
    assert_eq!(error.code, "chat_mutation_recovery_invalid");

    drop(engine);
    let engine = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    assert_workspace_still_works(&engine);
    assert!(garbage.exists() && escaping.exists() && future.exists());
    assert!(
        !workspace
            .path()
            .parent()
            .unwrap()
            .join("outside.md")
            .exists()
    );

    std::fs::remove_file(future).unwrap();
    engine.read_chat(&chat.value.id).unwrap();
}

#[test]
fn a_write_replays_under_the_lock_an_intent_it_did_not_see_before_the_lock() {
    let (workspace, _app_data, engine) = quiet_engine();
    let (chat_id, call) = chat_with_open_call(&engine);
    let chat_revision = engine.read_chat(&chat_id).unwrap().chat.revision;
    engine.fail_chat_mutation_write_for_testing("chat", 1);
    append_user(&engine, &chat_id, chat_revision).unwrap_err();
    let intent = intents(workspace.path()).remove(0);
    let intent_bytes = std::fs::read(&intent).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&intent_bytes).unwrap();
    let message = workspace
        .path()
        .join(parsed["message"]["relative_path"].as_str().unwrap());
    let message_bytes = std::fs::read(&message).unwrap();
    // Take the failed change away for now. The hook brings it back as
    // another writer leaves it, just after this recovery's read.
    std::fs::remove_file(&intent).unwrap();
    std::fs::remove_file(&message).unwrap();
    engine.reconcile().unwrap();
    let hook_intent = intent.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::AfterDecidingRead, move |_| {
        std::fs::write(&message, message_bytes).unwrap();
        std::fs::write(&hook_intent, intent_bytes).unwrap();
    });

    let error = engine.recover_interrupted_chat(&chat_id).unwrap_err();

    assert_eq!(error.code, "revision_conflict");
    assert!(intents(workspace.path()).is_empty());
    let read = engine.read_chat(&chat_id).unwrap();
    assert_eq!(read.messages.len(), 2);
    assert!(read.messages.iter().any(|message| {
        message.id == call.value.id && message.status == ChatMessageStatus::InProgress
    }));
}

#[test]
fn expiry_leaves_a_chat_whose_change_is_stuck() {
    let (workspace, _app_data, engine) = quiet_engine();
    let chat = engine
        .create_chat(CreateChatInput {
            title: "Cut off".into(),
            retention: Some(ChatRetention::Ephemeral),
            retention_days: Some(30),
        })
        .unwrap();
    let directory = workspace.path().join(".noura/chat-mutations");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("future.json"),
        serde_json::json!({
            "version": 2,
            "chat": { "relative_path": chat.value.relative_path, "expected_revision": null, "bytes": "x" },
            "message": { "relative_path": "chats/a/messages/b.md", "expected_revision": null, "bytes": "x" },
        })
        .to_string(),
    )
    .unwrap();

    let expired = engine.expire_chats("2099-01-01T00:00:00Z").unwrap();

    assert!(expired.is_empty());
    assert!(workspace.path().join(&chat.value.relative_path).exists());
    assert_eq!(intents(workspace.path()).len(), 1);
}

#[cfg(unix)]
#[test]
fn a_replay_the_disk_refuses_keeps_the_chat_readable_and_refuses_its_writes() {
    use std::os::unix::fs::PermissionsExt;

    let (workspace, _app_data, engine) = quiet_engine();
    let chat = create_chat(&engine, "Refused");
    let directory = workspace
        .path()
        .join(&chat.value.relative_path)
        .parent()
        .unwrap()
        .to_owned();
    engine.fail_chat_mutation_write_for_testing("message", 1);
    append_user(&engine, &chat.value.id, chat.revision.clone()).unwrap_err();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555)).unwrap();

    let read = engine.read_chat(&chat.value.id);
    let write = append_user(&engine, &chat.value.id, chat.revision.clone());
    assert_workspace_still_works(&engine);
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(read.unwrap().messages.is_empty());
    assert!(write.is_err());
    assert_eq!(intents(workspace.path()).len(), 1);
    assert_eq!(engine.read_chat(&chat.value.id).unwrap().messages.len(), 1);
    assert!(intents(workspace.path()).is_empty());
}

#[test]
fn a_reader_that_finds_no_intent_finds_the_index_marked_stale() {
    let (workspace, _app_data, engine) = quiet_engine();
    let (chat, _) = chat_cut_off_before_its_chat_file(workspace.path(), &engine);
    assert!(!engine.index_marked_stale_for_testing());
    let root = workspace.path().to_owned();
    let saw_the_moment = Arc::new(AtomicBool::new(false));
    let saw = saw_the_moment.clone();
    engine.on_chat_recovery_point(ChatRecoveryPoint::IntentRemoved, move |engine| {
        assert!(intents(&root).is_empty());
        assert!(
            engine.index_marked_stale_for_testing(),
            "no intent and no stale mark: a reader would trust an index behind the files"
        );
        saw.store(true, Ordering::SeqCst);
    });

    let read = engine.read_chat(&chat.value.id).unwrap();

    assert!(saw_the_moment.load(Ordering::SeqCst));
    assert_eq!(read.messages.len(), 1);
}

#[test]
fn a_failing_recovery_walks_once_until_a_file_changes_or_the_wait_ends() {
    let (workspace, _app_data, engine) = quiet_engine();
    let (chat_id, call) = chat_with_open_call(&engine);
    let call_path = workspace.path().join(&call.value.relative_path);
    engine.fail_chat_mutation_write_for_testing("intent", usize::MAX);

    let error = engine.recover_interrupted_chat(&chat_id).unwrap_err();
    assert_eq!(error.code, "chat_mutation_write_failed");

    // The failure keeps going, and the retries leave the workspace alone.
    let walks = engine.workspace_walks_for_testing();
    for _ in 0..5 {
        let read = engine.recover_interrupted_chat(&chat_id).unwrap();
        assert_eq!(read.messages[0].status, ChatMessageStatus::InProgress);
    }
    assert_eq!(engine.workspace_walks_for_testing(), walks);

    // A file changed: one more try, which walks and fails.
    std::fs::write(workspace.path().join("Changed.md"), "changed").unwrap();
    engine.reconcile().unwrap();
    let walks = engine.workspace_walks_for_testing();
    engine.recover_interrupted_chat(&chat_id).unwrap_err();
    assert_eq!(engine.workspace_walks_for_testing(), walks + 1);
    engine.recover_interrupted_chat(&chat_id).unwrap();
    assert_eq!(engine.workspace_walks_for_testing(), walks + 1);

    // The wait ended: one more try, which walks and fails.
    engine.age_failed_chat_recovery_for_testing(&chat_id, Duration::from_secs(60));
    engine.recover_interrupted_chat(&chat_id).unwrap_err();
    assert_eq!(engine.workspace_walks_for_testing(), walks + 2);

    // The disk recovers. The call settles on the first try after the wait,
    // with its result counted and its status durable.
    engine.fail_chat_mutation_write_for_testing("intent", 0);
    append_result(&engine, &chat_id);
    assert_eq!(
        status_of(&engine, &chat_id, &call.value.id),
        ChatMessageStatus::InProgress
    );
    engine.age_failed_chat_recovery_for_testing(&chat_id, Duration::from_secs(60));
    let recovered = engine.recover_interrupted_chat(&chat_id).unwrap();
    assert_eq!(
        recovered
            .messages
            .iter()
            .find(|message| message.id == call.value.id)
            .unwrap()
            .status,
        ChatMessageStatus::Completed
    );
    assert!(
        std::fs::read_to_string(call_path)
            .unwrap()
            .contains("status: completed")
    );
}
