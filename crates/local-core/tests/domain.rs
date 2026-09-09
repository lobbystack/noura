use std::{collections::BTreeMap, time::Duration};

use local_core::{
    AppendChatContextSummaryInput, AppendChatToolResultInput, AppendChatUserMessageInput,
    BeginChatAssistantInput, BeginChatToolCallInput, ChangeChatRetentionInput, ChatMessageStatus,
    ChatRetention, ConflictResolution, CreateChatInput, CreateObjectInput,
    FinishChatAssistantInput, FinishChatToolCallInput, ManagedConflictResolution,
    ManagedConflictResolveInput, ManagedDraftInput, ManagedDraftResult, ManifestUpdateInput,
    MarkdownLinkTarget, ObjectPatch, ParseStatus, ParsedMarkdown, RawConflictResolveInput,
    RawSaveInput, RawSaveResult, RenameChatInput, SearchInput, WorkspaceEngine, WorkspaceEntryKind,
    WorkspaceManifest, new_object_id, parse_markdown,
};
use tempfile::tempdir;

fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Domain tests", app_data.path())
            .unwrap();
    (workspace, app_data, engine)
}

fn replace_frontmatter_timestamp(path: &std::path::Path, field: &str, value: &str) {
    let bytes = std::fs::read_to_string(path).unwrap();
    let mut replaced = false;
    let rewritten = bytes
        .lines()
        .map(|line| {
            if line.starts_with(&format!("{field}: ")) {
                replaced = true;
                format!("{field}: {value}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(replaced, "{field} frontmatter field must be present");
    std::fs::write(path, format!("{rewritten}\n")).unwrap();
}

#[test]
fn chats_are_canonical_revision_checked_and_rebuildable() {
    let (workspace, app_data, engine) = engine();
    let created = engine
        .create_chat(CreateChatInput {
            title: "Release planning".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap();
    assert!(
        created
            .value
            .relative_path
            .starts_with("chats/release-planning--")
    );
    assert!(created.value.relative_path.ends_with("/chat.md"));

    let user = engine
        .append_chat_user_message(AppendChatUserMessageInput {
            chat_id: created.value.id.clone(),
            run_id: "run_release".into(),
            content: "What changed?".into(),
            expected_chat_revision: created.revision,
        })
        .unwrap();
    assert!(user.value.relative_path.contains("/messages/"));
    assert!(
        user.value
            .relative_path
            .ends_with(&format!("{}.md", user.value.id))
    );
    let after_user = engine.read_chat(&created.value.id).unwrap();

    let assistant = engine
        .begin_chat_assistant(BeginChatAssistantInput {
            chat_id: created.value.id.clone(),
            run_id: "run_release".into(),
            provider_id: "openai".into(),
            model_id: "gpt-5.6".into(),
            expected_chat_revision: after_user.chat.revision,
        })
        .unwrap();
    let after_begin = engine.read_chat(&created.value.id).unwrap();
    let _assistant = engine
        .finish_chat_assistant(FinishChatAssistantInput {
            chat_id: created.value.id.clone(),
            message_id: assistant.value.id,
            content: "The release is ready.".into(),
            status: ChatMessageStatus::Completed,
            error_code: None,
            expected_chat_revision: after_begin.chat.revision,
            expected_message_revision: assistant.revision,
        })
        .unwrap();
    let after_assistant = engine.read_chat(&created.value.id).unwrap();

    let tool_call = engine
        .begin_chat_tool_call(BeginChatToolCallInput {
            chat_id: created.value.id.clone(),
            run_id: "run_release".into(),
            tool_call_id: "call_release".into(),
            tool_name: "workspace.search".into(),
            content: "{\"query\":\"release\"}".into(),
            expected_chat_revision: after_assistant.chat.revision,
        })
        .unwrap();
    let after_tool_begin = engine.read_chat(&created.value.id).unwrap();
    engine
        .finish_chat_tool_call(FinishChatToolCallInput {
            chat_id: created.value.id.clone(),
            message_id: tool_call.value.id.clone(),
            content: r#"{"status":"invoked"}"#.into(),
            status: ChatMessageStatus::Completed,
            error_code: None,
            expected_chat_revision: after_tool_begin.chat.revision,
            expected_message_revision: tool_call.revision,
        })
        .unwrap();
    let after_tool_call = engine.read_chat(&created.value.id).unwrap();
    engine
        .append_chat_tool_result(AppendChatToolResultInput {
            chat_id: created.value.id.clone(),
            run_id: "run_release".into(),
            tool_call_id: "call_release".into(),
            tool_name: "workspace.search".into(),
            content: r#"{"result":"Found release notes"}"#.into(),
            expected_chat_revision: after_tool_call.chat.revision,
        })
        .unwrap();
    let after_tool_result = engine.read_chat(&created.value.id).unwrap();
    engine
        .append_chat_context_summary(AppendChatContextSummaryInput {
            chat_id: created.value.id.clone(),
            run_id: "run_release".into(),
            content: "The release discussion is ready to continue.".into(),
            summarizes_through_message_id: tool_call.value.id.clone(),
            expected_chat_revision: after_tool_result.chat.revision,
        })
        .unwrap();
    let read = engine.read_chat(&created.value.id).unwrap();
    assert_eq!(read.messages.len(), 5);
    assert_eq!(read.messages[0].content, "What changed?");
    assert_eq!(read.messages[1].content, "The release is ready.");
    assert!(
        read.messages
            .iter()
            .all(|message| message.status == ChatMessageStatus::Completed)
    );

    let stale = engine
        .append_chat_user_message(AppendChatUserMessageInput {
            chat_id: created.value.id.clone(),
            run_id: "run_release".into(),
            content: "stale".into(),
            expected_chat_revision: created.value.revision,
        })
        .unwrap_err();
    assert_eq!(stale.code, "revision_conflict");

    let index_path = engine.index_path().to_owned();
    drop(engine);
    std::fs::remove_file(index_path).unwrap();
    let rebuilt = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    let rebuilt_read = rebuilt.read_chat(&created.value.id).unwrap();
    assert_eq!(rebuilt_read.messages.len(), 5);
    assert_eq!(
        rebuilt_read.messages[4].content,
        "The release discussion is ready to continue."
    );
}

#[test]
fn chat_mutation_intents_recover_each_write_failure_across_restart_and_rebuild() {
    for target in ["message", "chat"] {
        let (workspace, app_data, engine) = engine();
        let created = engine
            .create_chat(CreateChatInput {
                title: format!("Recover {target}"),
                retention: None,
                retention_days: None,
            })
            .unwrap();
        let mut events = engine.subscribe();
        engine.fail_chat_mutation_write_for_testing(target, 1);
        let error = engine
            .append_chat_user_message(AppendChatUserMessageInput {
                chat_id: created.value.id.clone(),
                run_id: "run_recovery".into(),
                content: format!("recover after {target} write failure"),
                expected_chat_revision: created.revision.clone(),
            })
            .unwrap_err();
        assert_eq!(error.code, "chat_mutation_write_failed");
        assert!(
            events.try_recv().is_err(),
            "failed commits must not emit chat events"
        );

        let index_path = engine.index_path().to_owned();
        drop(engine);
        std::fs::remove_file(index_path).unwrap();
        let reopened =
            WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
        let recovered = reopened.read_chat(&created.value.id).unwrap();
        assert_eq!(
            recovered.messages.len(),
            1,
            "failed {target} write was not recovered"
        );
        assert_eq!(
            recovered.messages[0].content,
            format!("recover after {target} write failure")
        );
        assert!(
            !workspace.path().join(".noura/chat-mutations").exists()
                || workspace
                    .path()
                    .join(".noura/chat-mutations")
                    .read_dir()
                    .unwrap()
                    .next()
                    .is_none()
        );

        let stale = reopened
            .append_chat_user_message(AppendChatUserMessageInput {
                chat_id: created.value.id.clone(),
                run_id: "run_stale".into(),
                content: "must not replay as a new message".into(),
                expected_chat_revision: created.revision.clone(),
            })
            .unwrap_err();
        assert_eq!(stale.code, "revision_conflict");
    }
}

#[test]
fn chat_mutation_recovery_does_not_clobber_an_external_parent_edit() {
    let (workspace, _app_data, engine) = engine();
    let created = engine
        .create_chat(CreateChatInput {
            title: "Do not overwrite".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap();
    engine.fail_chat_mutation_write_for_testing("chat", 1);
    let error = engine
        .append_chat_user_message(AppendChatUserMessageInput {
            chat_id: created.value.id.clone(),
            run_id: "run_external".into(),
            content: "the child write completed first".into(),
            expected_chat_revision: created.revision,
        })
        .unwrap_err();
    assert_eq!(error.code, "chat_mutation_write_failed");

    let chat_path = workspace.path().join(&created.value.relative_path);
    let external = std::fs::read_to_string(&chat_path)
        .unwrap()
        .replace("title: Do not overwrite", "title: Edited outside Noura");
    std::fs::write(&chat_path, external).unwrap();

    let recovery = engine.reconcile().unwrap_err();
    assert_eq!(recovery.code, "chat_mutation_recovery_conflict");
    assert!(
        std::fs::read_to_string(chat_path)
            .unwrap()
            .contains("title: Edited outside Noura")
    );
    assert!(
        workspace
            .path()
            .join(".noura/chat-mutations")
            .read_dir()
            .unwrap()
            .next()
            .is_some(),
        "the unresolved intent must remain available for explicit recovery"
    );
}

#[test]
fn chat_retention_changes_are_revision_checked_and_durable() {
    let (workspace, _app_data, engine) = engine();
    let created = engine
        .create_chat(CreateChatInput {
            title: "Retention".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap();
    let mut events = engine.subscribe();
    let changed = engine
        .change_chat_retention(ChangeChatRetentionInput {
            chat_id: created.value.id.clone(),
            retention: ChatRetention::Ephemeral,
            retention_days: Some(30),
            expected_chat_revision: created.revision.clone(),
        })
        .unwrap();
    assert_eq!(changed.value.retention, ChatRetention::Ephemeral);
    assert_eq!(changed.value.retention_days, Some(30));
    assert_ne!(changed.revision, created.revision);
    assert_eq!(
        events.try_recv().unwrap().event_type,
        "chat:retention-changed"
    );

    let bytes = std::fs::read(workspace.path().join(&changed.value.relative_path)).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("retention: ephemeral\nretention_days: 30\n"));

    let invalid = engine
        .change_chat_retention(ChangeChatRetentionInput {
            chat_id: changed.value.id.clone(),
            retention: ChatRetention::Ephemeral,
            retention_days: None,
            expected_chat_revision: changed.revision.clone(),
        })
        .unwrap_err();
    assert_eq!(invalid.code, "invalid_chat_markdown");

    let permanent = engine
        .change_chat_retention(ChangeChatRetentionInput {
            chat_id: changed.value.id.clone(),
            retention: ChatRetention::Permanent,
            retention_days: None,
            expected_chat_revision: changed.revision,
        })
        .unwrap();
    assert_eq!(permanent.value.retention, ChatRetention::Permanent);
    assert_eq!(permanent.value.retention_days, None);

    let stale = engine
        .change_chat_retention(ChangeChatRetentionInput {
            chat_id: created.value.id,
            retention: ChatRetention::Permanent,
            retention_days: None,
            expected_chat_revision: created.revision,
        })
        .unwrap_err();
    assert_eq!(stale.code, "revision_conflict");
}

#[test]
fn chat_rename_preserves_path_and_is_revision_checked_and_durable() {
    let (workspace, _app_data, engine) = engine();
    let created = engine
        .create_chat(CreateChatInput {
            title: "Initial title".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap();
    let path = created.value.relative_path.clone();
    let mut events = engine.subscribe();

    let renamed = engine
        .rename_chat(RenameChatInput {
            chat_id: created.value.id.clone(),
            title: "  Renamed chat  ".into(),
            expected_chat_revision: created.revision.clone(),
        })
        .unwrap();
    assert_eq!(renamed.value.title, "Renamed chat");
    assert_eq!(renamed.value.relative_path, path);
    assert_ne!(renamed.revision, created.revision);
    assert_eq!(events.try_recv().unwrap().event_type, "chat:renamed");

    let bytes = std::fs::read(workspace.path().join(&renamed.value.relative_path)).unwrap();
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap()
            .contains("title: Renamed chat\n")
    );

    let invalid = engine
        .rename_chat(RenameChatInput {
            chat_id: renamed.value.id.clone(),
            title: "Renamed\nchat".into(),
            expected_chat_revision: renamed.revision.clone(),
        })
        .unwrap_err();
    assert_eq!(invalid.code, "chat_title_required");

    let stale = engine
        .rename_chat(RenameChatInput {
            chat_id: renamed.value.id,
            title: "Stale title".into(),
            expected_chat_revision: created.revision,
        })
        .unwrap_err();
    assert_eq!(stale.code, "revision_conflict");
}

#[test]
fn chat_finishes_persist_terminal_statuses_and_partial_content() {
    let (_workspace, _app_data, engine) = engine();
    let chat = engine
        .create_chat(CreateChatInput {
            title: "Terminal states".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap();
    let cancelled = engine
        .begin_chat_assistant(BeginChatAssistantInput {
            chat_id: chat.value.id.clone(),
            run_id: "run_terminal".into(),
            provider_id: "openai".into(),
            model_id: "gpt-5.6".into(),
            expected_chat_revision: chat.revision,
        })
        .unwrap();
    let after_cancelled_begin = engine.read_chat(&chat.value.id).unwrap();
    let cancelled = engine
        .finish_chat_assistant(FinishChatAssistantInput {
            chat_id: chat.value.id.clone(),
            message_id: cancelled.value.id,
            content: "Partial response".into(),
            status: ChatMessageStatus::Cancelled,
            error_code: None,
            expected_chat_revision: after_cancelled_begin.chat.revision,
            expected_message_revision: cancelled.revision,
        })
        .unwrap();
    assert_eq!(cancelled.value.status, ChatMessageStatus::Cancelled);
    assert_eq!(cancelled.value.content, "Partial response");
    let after_cancelled = engine.read_chat(&chat.value.id).unwrap();

    let invalid_cancelled = engine
        .begin_chat_assistant(BeginChatAssistantInput {
            chat_id: chat.value.id.clone(),
            run_id: "run_terminal".into(),
            provider_id: "openai".into(),
            model_id: "gpt-5.6".into(),
            expected_chat_revision: after_cancelled.chat.revision.clone(),
        })
        .unwrap();
    let after_invalid_cancelled_begin = engine.read_chat(&chat.value.id).unwrap();
    let invalid_cancelled = engine
        .finish_chat_assistant(FinishChatAssistantInput {
            chat_id: chat.value.id.clone(),
            message_id: invalid_cancelled.value.id,
            content: "Partial response".into(),
            status: ChatMessageStatus::Cancelled,
            error_code: Some("cancelled".into()),
            expected_chat_revision: after_invalid_cancelled_begin.chat.revision,
            expected_message_revision: invalid_cancelled.revision,
        })
        .unwrap_err();
    assert_eq!(invalid_cancelled.code, "invalid_chat_markdown");
    let after_cancelled = engine.read_chat(&chat.value.id).unwrap();

    let failed = engine
        .begin_chat_assistant(BeginChatAssistantInput {
            chat_id: chat.value.id.clone(),
            run_id: "run_terminal".into(),
            provider_id: "openai".into(),
            model_id: "gpt-5.6".into(),
            expected_chat_revision: after_cancelled.chat.revision,
        })
        .unwrap();
    let after_failed_begin = engine.read_chat(&chat.value.id).unwrap();
    let unsafe_error = engine
        .finish_chat_assistant(FinishChatAssistantInput {
            chat_id: chat.value.id.clone(),
            message_id: failed.value.id.clone(),
            content: "Provider stopped after partial text".into(),
            status: ChatMessageStatus::Failed,
            error_code: Some("provider failure".into()),
            expected_chat_revision: after_failed_begin.chat.revision.clone(),
            expected_message_revision: failed.revision.clone(),
        })
        .unwrap_err();
    assert_eq!(unsafe_error.code, "invalid_chat_markdown");
    let failed = engine
        .finish_chat_assistant(FinishChatAssistantInput {
            chat_id: chat.value.id.clone(),
            message_id: failed.value.id,
            content: "Provider stopped after partial text".into(),
            status: ChatMessageStatus::Failed,
            error_code: Some("provider_unavailable".into()),
            expected_chat_revision: after_failed_begin.chat.revision,
            expected_message_revision: failed.revision,
        })
        .unwrap();
    assert_eq!(failed.value.status, ChatMessageStatus::Failed);
    assert_eq!(
        failed.value.error_code.as_deref(),
        Some("provider_unavailable")
    );
    let after_failed = engine.read_chat(&chat.value.id).unwrap();

    let tool_call = engine
        .begin_chat_tool_call(BeginChatToolCallInput {
            chat_id: chat.value.id.clone(),
            run_id: "run_terminal".into(),
            tool_call_id: "call_terminal".into(),
            tool_name: "workspace.search".into(),
            content: r#"{"query":"terminal"}"#.into(),
            expected_chat_revision: after_failed.chat.revision,
        })
        .unwrap();
    let after_tool_begin = engine.read_chat(&chat.value.id).unwrap();
    let non_terminal = engine
        .finish_chat_tool_call(FinishChatToolCallInput {
            chat_id: chat.value.id.clone(),
            message_id: tool_call.value.id.clone(),
            content: r#"{"retry":true}"#.into(),
            status: ChatMessageStatus::InProgress,
            error_code: None,
            expected_chat_revision: after_tool_begin.chat.revision.clone(),
            expected_message_revision: tool_call.revision.clone(),
        })
        .unwrap_err();
    assert_eq!(non_terminal.code, "chat_message_not_terminal");
    let tool_call = engine
        .finish_chat_tool_call(FinishChatToolCallInput {
            chat_id: chat.value.id,
            message_id: tool_call.value.id,
            content: r#"{"cancelled":true}"#.into(),
            status: ChatMessageStatus::Interrupted,
            error_code: Some("plugin-disabled".into()),
            expected_chat_revision: after_tool_begin.chat.revision,
            expected_message_revision: tool_call.revision,
        })
        .unwrap();
    assert_eq!(tool_call.value.status, ChatMessageStatus::Interrupted);
    assert_eq!(
        tool_call.value.error_code.as_deref(),
        Some("plugin-disabled")
    );
}

#[test]
fn chat_recovery_marks_interrupted_messages_and_retention_trashes_only_safe_directories() {
    let (workspace, _app_data, engine) = engine();
    let chat = engine
        .create_chat(CreateChatInput {
            title: "Recovery".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap()
        .value;
    let pending = engine
        .begin_chat_assistant(BeginChatAssistantInput {
            chat_id: chat.id.clone(),
            run_id: "run_recovery".into(),
            provider_id: "openai".into(),
            model_id: "gpt-5.6".into(),
            expected_chat_revision: chat.revision.clone(),
        })
        .unwrap();
    let recovered = engine.recover_interrupted_chat(&chat.id).unwrap();
    assert_eq!(recovered.messages.len(), 1);
    assert_eq!(recovered.messages[0].id, pending.value.id);
    assert_eq!(recovered.messages[0].status, ChatMessageStatus::Interrupted);

    let retained = engine
        .create_chat(CreateChatInput {
            title: "Keep unknown files".into(),
            retention: Some(ChatRetention::Ephemeral),
            retention_days: Some(30),
        })
        .unwrap()
        .value;
    let retained_dir = workspace
        .path()
        .join(&retained.relative_path)
        .parent()
        .unwrap()
        .to_owned();
    std::fs::write(retained_dir.join("manual.txt"), "do not move").unwrap();
    assert!(
        engine
            .expire_chats("2031-01-01T00:00:00Z")
            .unwrap()
            .is_empty()
    );
    assert!(retained_dir.exists());

    let expiring = engine
        .create_chat(CreateChatInput {
            title: "Expire safely".into(),
            retention: Some(ChatRetention::Ephemeral),
            retention_days: Some(30),
        })
        .unwrap()
        .value;
    let expired = engine.expire_chats("2031-01-01T00:00:00Z").unwrap();
    assert_eq!(expired, vec![expiring.id]);
    assert!(!workspace.path().join(expiring.relative_path).exists());
    assert!(
        workspace
            .path()
            .join(".noura/trash")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
}

#[test]
fn chat_recovery_completes_tool_calls_with_matching_durable_results() {
    let (workspace, app_data, engine) = engine();
    let chat = engine
        .create_chat(CreateChatInput {
            title: "Recovery tool result".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap();
    let tool_call = engine
        .begin_chat_tool_call(BeginChatToolCallInput {
            chat_id: chat.value.id.clone(),
            run_id: "run_recovery".into(),
            tool_call_id: "call_recovery".into(),
            tool_name: "workspace.search".into(),
            content: r#"{"query":"status"}"#.into(),
            expected_chat_revision: chat.revision,
        })
        .unwrap();
    let after_call = engine.read_chat(&chat.value.id).unwrap();
    let tool_result = engine
        .append_chat_tool_result(AppendChatToolResultInput {
            chat_id: chat.value.id.clone(),
            run_id: "run_recovery".into(),
            tool_call_id: "call_recovery".into(),
            tool_name: "workspace.search".into(),
            content: r#"{"matches":1}"#.into(),
            expected_chat_revision: after_call.chat.revision,
        })
        .unwrap();

    drop(engine);
    let reopened = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    let recovered = reopened.recover_interrupted_chat(&chat.value.id).unwrap();
    let recovered_call = recovered
        .messages
        .iter()
        .find(|message| message.id == tool_call.value.id)
        .unwrap();
    assert_eq!(recovered_call.status, ChatMessageStatus::Completed);
    assert_eq!(recovered_call.content, r#"{"query":"status"}"#);
    assert!(recovered.messages.iter().any(|message| {
        message.id == tool_result.value.id && message.status == ChatMessageStatus::Completed
    }));
}

#[test]
fn chat_recovery_interrupts_tool_calls_without_matching_durable_results() {
    let (_workspace, _app_data, engine) = engine();
    let chat = engine
        .create_chat(CreateChatInput {
            title: "Unmatched recovery result".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap();
    let tool_call = engine
        .begin_chat_tool_call(BeginChatToolCallInput {
            chat_id: chat.value.id.clone(),
            run_id: "run_recovery".into(),
            tool_call_id: "call_pending".into(),
            tool_name: "workspace.search".into(),
            content: r#"{"query":"status"}"#.into(),
            expected_chat_revision: chat.revision,
        })
        .unwrap();
    let after_call = engine.read_chat(&chat.value.id).unwrap();
    engine
        .append_chat_tool_result(AppendChatToolResultInput {
            chat_id: chat.value.id.clone(),
            run_id: "run_other".into(),
            tool_call_id: "call_pending".into(),
            tool_name: "workspace.list".into(),
            content: r#"{"entries":[]}"#.into(),
            expected_chat_revision: after_call.chat.revision,
        })
        .unwrap();

    let recovered = engine.recover_interrupted_chat(&chat.value.id).unwrap();
    assert_eq!(
        recovered
            .messages
            .iter()
            .find(|message| message.id == tool_call.value.id)
            .unwrap()
            .status,
        ChatMessageStatus::Interrupted
    );
}

#[test]
fn chat_reads_sort_rfc3339_offsets_by_instant() {
    let (workspace, _app_data, engine) = engine();
    let earlier_chat = engine
        .create_chat(CreateChatInput {
            title: "Earlier instant".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap()
        .value;
    let later_chat = engine
        .create_chat(CreateChatInput {
            title: "Later instant".into(),
            retention: None,
            retention_days: None,
        })
        .unwrap()
        .value;
    replace_frontmatter_timestamp(
        &workspace.path().join(&earlier_chat.relative_path),
        "updated",
        "2026-01-01T00:30:00+01:00",
    );
    replace_frontmatter_timestamp(
        &workspace.path().join(&later_chat.relative_path),
        "updated",
        "2025-12-31T23:45:00Z",
    );

    let chats = engine.list_chats().unwrap();
    let ordered_ids = chats
        .iter()
        .map(|chat| chat.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ordered_ids[..2],
        [later_chat.id.as_str(), earlier_chat.id.as_str()]
    );

    let first = engine
        .append_chat_user_message(AppendChatUserMessageInput {
            chat_id: later_chat.id.clone(),
            run_id: "run_sort".into(),
            content: "First instant".into(),
            expected_chat_revision: chats
                .iter()
                .find(|chat| chat.id == later_chat.id)
                .unwrap()
                .revision
                .clone(),
        })
        .unwrap();
    let after_first = engine.read_chat(&later_chat.id).unwrap();
    let second = engine
        .append_chat_user_message(AppendChatUserMessageInput {
            chat_id: later_chat.id.clone(),
            run_id: "run_sort".into(),
            content: "Second instant".into(),
            expected_chat_revision: after_first.chat.revision,
        })
        .unwrap();
    replace_frontmatter_timestamp(
        &workspace.path().join(&first.value.relative_path),
        "created",
        "2026-01-01T00:30:00+01:00",
    );
    replace_frontmatter_timestamp(
        &workspace.path().join(&second.value.relative_path),
        "created",
        "2025-12-31T23:45:00Z",
    );

    let messages = engine.read_chat(&later_chat.id).unwrap().messages;
    let ordered_ids = messages
        .iter()
        .map(|message| message.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ordered_ids,
        [first.value.id.as_str(), second.value.id.as_str()]
    );
}

#[test]
fn managed_draft_save_merges_disjoint_fields_and_snapshots_external_version() {
    let (workspace, app_data, engine) = engine();
    let created = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Ship beta".into(),
            body: "first opening\n\nsecond line detail".into(),
            relative_path: None,
            properties: BTreeMap::from([
                ("status".into(), serde_json::json!("todo")),
                ("priority".into(), serde_json::json!("medium")),
            ]),
        })
        .unwrap();
    let base = created.value;
    let external = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    external
        .update_object(
            &base.id,
            ObjectPatch {
                title: None,
                body: Some("first opening\n\nsecond line detail from file".into()),
                properties: BTreeMap::from([("priority".into(), serde_json::json!("high"))]),
                remove_properties: Vec::new(),
                expected_revision: base.revision.clone(),
            },
        )
        .unwrap();
    let local = ManagedDraftInput {
        id: base.id.clone(),
        base_revision: base.revision.clone(),
        base_title: base.title.clone(),
        base_body: "first opening\n\nsecond line detail".into(),
        base_properties: base.properties.clone(),
        local_title: base.title.clone(),
        local_body: "first opening edited locally\n\nsecond line detail".into(),
        local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
    };
    let result = engine.save_managed_draft(local).unwrap();
    let ManagedDraftResult::Merged {
        current,
        title,
        body,
        properties,
    } = result
    else {
        panic!("expected merged draft, got {result:?}");
    };
    assert_eq!(title, "Ship beta");
    assert_eq!(
        body,
        "first opening edited locally\n\nsecond line detail from file",
    );
    assert_eq!(properties["status"], serde_json::json!("in-progress"));
    assert_eq!(properties["priority"], serde_json::json!("high"));
    assert_ne!(current.revision, base.revision);
    assert!(
        workspace
            .path()
            .join(".noura/history")
            .join(&base.id)
            .read_dir()
            .unwrap()
            .next()
            .is_some(),
        "external snapshot should be recoverable"
    );
}

#[test]
fn managed_draft_save_updates_an_unchanged_canonical_file() {
    let (workspace, _app_data, engine) = engine();
    let base = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Original".into(),
            body: "base body".into(),
            relative_path: None,
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;

    let result = engine
        .save_managed_draft(ManagedDraftInput {
            id: base.id.clone(),
            base_revision: base.revision.clone(),
            base_title: base.title.clone(),
            base_body: base.body.clone(),
            base_properties: base.properties.clone(),
            local_title: "Renamed".into(),
            local_body: "edited body".into(),
            local_properties: BTreeMap::from([
                ("status".into(), serde_json::json!("in-progress")),
                ("priority".into(), serde_json::json!("high")),
            ]),
        })
        .unwrap();
    let ManagedDraftResult::Unchanged { current } = result else {
        panic!("expected clean-base save, got {result:?}");
    };

    assert_eq!(current.title, "Renamed");
    assert_eq!(current.body, "edited body");
    assert_eq!(current.properties["status"], "in-progress");
    assert_eq!(current.properties["priority"], "high");
    assert_ne!(current.revision, base.revision);
    assert!(workspace.path().join(&current.relative_path).exists());
}

#[test]
fn notes_support_create_update_move_and_trash() {
    let (workspace, _app_data, engine) = engine();
    let created = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Lifecycle".into(),
            body: "First".into(),
            relative_path: None,
            properties: BTreeMap::new(),
        })
        .unwrap();
    let updated = engine
        .update_object(
            &created.value.id,
            ObjectPatch {
                title: Some("Renamed".into()),
                body: Some("Second".into()),
                properties: BTreeMap::from([("custom".into(), serde_json::json!(true))]),
                remove_properties: Vec::new(),
                expected_revision: created.revision,
            },
        )
        .unwrap();
    let moved = engine
        .move_object(&updated.value.id, "archive/note.md", &updated.revision)
        .unwrap();
    let deleted = engine
        .delete_object(&moved.value.id, &moved.revision)
        .unwrap();
    assert_eq!(deleted.value.id, created.value.id);
    assert!(!workspace.path().join("archive/note.md").exists());
    assert!(
        workspace
            .path()
            .join(".noura/trash")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
}

#[test]
fn idless_markdown_requires_explicit_adoption() {
    let (workspace, _app_data, engine) = engine();
    let path = workspace.path().join("draft.md");
    std::fs::write(&path, "# Draft\n\nText\n").unwrap();
    engine.reconcile().unwrap();
    assert!(engine.query_objects(Some("note")).unwrap().is_empty());
    assert_eq!(
        engine
            .search(&SearchInput {
                query: "Draft".into(),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
    let bytes = std::fs::read(&path).unwrap();
    let revision = blake3::hash(&bytes).to_hex().to_string();
    let adopted = engine
        .adopt_markdown("draft.md", "note", &revision)
        .unwrap();
    assert!(matches!(
        parse_markdown("draft.md", &std::fs::read(path).unwrap()),
        ParsedMarkdown::Managed(_)
    ));
    assert_eq!(adopted.value.title, "Draft");
}

#[test]
fn tasks_projects_calendar_and_kanban_metadata_share_files() {
    let (_workspace, _app_data, engine) = engine();
    let project = engine
        .create_object(CreateObjectInput {
            object_type: "project".into(),
            title: "Launch".into(),
            body: String::new(),
            relative_path: None,
            properties: BTreeMap::new(),
        })
        .unwrap();
    let task = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Ship".into(),
            body: "Checklist".into(),
            relative_path: None,
            properties: BTreeMap::from([
                ("project".into(), serde_json::json!(project.value.id)),
                ("due".into(), serde_json::json!("2026-09-04")),
                ("kanban_order".into(), serde_json::json!("a0")),
            ]),
        })
        .unwrap();
    assert_eq!(task.value.properties["status"], "todo");
    assert_eq!(task.value.properties["priority"], "medium");
    assert_eq!(
        engine.calendar("2026-09-01", "2026-10-01").unwrap()[0].source_id,
        task.value.id
    );
    let completed = engine
        .update_object(
            &task.value.id,
            ObjectPatch {
                title: None,
                body: None,
                properties: BTreeMap::from([
                    ("status".into(), serde_json::json!("done")),
                    ("kanban_order".into(), serde_json::json!("z0")),
                ]),
                remove_properties: Vec::new(),
                expected_revision: task.revision,
            },
        )
        .unwrap();
    assert_eq!(completed.value.properties["status"], "done");
    engine
        .delete_object(&project.value.id, &project.revision)
        .unwrap();
    assert!(engine.get_object(&task.value.id).unwrap().is_some());
}

#[test]
fn duplicate_ids_are_indexed_but_block_identity_reads() {
    let (workspace, _app_data, engine) = engine();
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "One".into(),
            body: String::new(),
            relative_path: Some("one.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    std::fs::copy(
        workspace.path().join("one.md"),
        workspace.path().join("two.md"),
    )
    .unwrap();
    engine.reconcile().unwrap();
    let error = engine.get_object(&note.value.id).unwrap_err();
    assert_eq!(error.code, "identity_conflict");
}

#[test]
fn reconciliation_repairs_an_external_save() {
    let (workspace, _app_data, engine) = engine();
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Watch".into(),
            body: "Before".into(),
            relative_path: Some("watch.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    let path = workspace.path().join("watch.md");
    let content = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Before", "After");
    std::fs::write(&path, content).unwrap();
    engine.reconcile().unwrap();
    assert_eq!(
        engine.get_object(&note.value.id).unwrap().unwrap().body,
        "After"
    );
}

#[test]
fn watcher_poll_accepts_an_empty_batch_and_keeps_the_workspace_usable() {
    let (_workspace, _app_data, engine) = engine();
    let changes = engine
        .poll_external_changes(Duration::from_millis(25))
        .unwrap();
    assert!(changes.is_empty());
    assert_eq!(engine.state().phase, local_core::WorkspacePhase::Ready);
}

#[test]
fn path_traversal_is_rejected_before_a_write() {
    let (_workspace, _app_data, engine) = engine();
    let error = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Unsafe".into(),
            body: String::new(),
            relative_path: Some("../outside.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap_err();
    assert_eq!(error.code, "unsafe_path");
}

#[test]
fn workspace_entries_classify_visible_workspace_content() {
    let (workspace, _app_data, engine) = engine();
    let managed = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Managed".into(),
            body: String::new(),
            relative_path: Some("docs/managed.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    std::fs::create_dir(workspace.path().join("archive")).unwrap();
    std::fs::write(workspace.path().join("docs/draft.md"), "# Draft\n\nBody\n").unwrap();
    std::fs::write(
        workspace.path().join("docs/broken.md"),
        "---\nid: incomplete\n# Broken\n",
    )
    .unwrap();
    std::fs::write(workspace.path().join("docs/image.png"), [0_u8, 1, 2]).unwrap();

    let entries = engine.list_workspace_entries().unwrap();
    let summary = entries
        .iter()
        .map(|entry| {
            (
                entry.relative_path.as_str(),
                entry.kind,
                entry.parse_status,
                entry.object_type.as_deref(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        summary,
        vec![
            ("archive", WorkspaceEntryKind::Folder, None, None),
            ("docs", WorkspaceEntryKind::Folder, None, None),
            (
                "docs/broken.md",
                WorkspaceEntryKind::File,
                Some(ParseStatus::Malformed),
                None,
            ),
            (
                "docs/draft.md",
                WorkspaceEntryKind::File,
                Some(ParseStatus::Unmanaged),
                None,
            ),
            (
                "docs/image.png",
                WorkspaceEntryKind::File,
                Some(ParseStatus::Binary),
                None,
            ),
            (
                "docs/managed.md",
                WorkspaceEntryKind::File,
                Some(ParseStatus::Managed),
                Some("note"),
            ),
        ]
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.relative_path == "docs/managed.md")
            .and_then(|entry| entry.object_id.as_deref()),
        Some(managed.value.id.as_str())
    );
}

#[test]
fn workspace_entries_apply_manifest_ignores_and_reserved_paths() {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Visibility", app_data.path())
            .unwrap();
    drop(engine);
    let manifest_path = workspace.path().join("workspace.yaml");
    let mut manifest: WorkspaceManifest =
        serde_yaml_ng::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest.ignore = vec!["ignored/".into()];
    std::fs::write(&manifest_path, serde_yaml_ng::to_string(&manifest).unwrap()).unwrap();
    std::fs::create_dir_all(workspace.path().join("ignored")).unwrap();
    std::fs::write(workspace.path().join("ignored/hidden.txt"), "hidden").unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
    std::fs::write(workspace.path().join(".git/config"), "hidden").unwrap();
    std::fs::write(workspace.path().join("visible.txt"), "visible").unwrap();
    let engine = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();

    let paths = engine
        .list_workspace_entries()
        .unwrap()
        .into_iter()
        .map(|entry| entry.relative_path)
        .collect::<Vec<_>>();

    assert_eq!(paths, vec!["visible.txt"]);
}

#[test]
fn workspace_entries_hide_dot_prefixed_paths() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(workspace.path().join(".DS_Store"), "metadata").unwrap();
    std::fs::create_dir_all(workspace.path().join(".metadata")).unwrap();
    std::fs::write(workspace.path().join(".metadata/preferences.json"), "{}").unwrap();
    std::fs::write(workspace.path().join("visible.txt"), "visible").unwrap();

    let paths = engine
        .list_workspace_entries()
        .unwrap()
        .into_iter()
        .map(|entry| entry.relative_path)
        .collect::<Vec<_>>();

    assert_eq!(paths, vec!["visible.txt"]);
}

#[test]
fn external_move_preserves_managed_identity_in_workspace_entries() {
    let (workspace, _app_data, engine) = engine();
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Movable".into(),
            body: String::new(),
            relative_path: Some("before.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    std::fs::create_dir(workspace.path().join("after")).unwrap();
    std::fs::rename(
        workspace.path().join("before.md"),
        workspace.path().join("after/moved.md"),
    )
    .unwrap();

    engine.reconcile().unwrap();
    let entries = engine.list_workspace_entries().unwrap();

    assert_eq!(
        (
            entries
                .iter()
                .any(|entry| entry.relative_path == "before.md"),
            entries
                .iter()
                .find(|entry| entry.relative_path == "after/moved.md")
                .and_then(|entry| entry.object_id.as_deref()),
        ),
        (false, Some(note.value.id.as_str()))
    );
}

#[test]
fn external_delete_disappears_from_entries_and_index() {
    let (workspace, _app_data, engine) = engine();
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Temporary".into(),
            body: String::new(),
            relative_path: Some("temporary.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    std::fs::remove_file(workspace.path().join("temporary.md")).unwrap();

    engine.reconcile().unwrap();

    assert_eq!(
        (
            engine
                .list_workspace_entries()
                .unwrap()
                .iter()
                .any(|entry| entry.relative_path == "temporary.md"),
            engine.get_object(&note.value.id).unwrap().is_some(),
        ),
        (false, false)
    );
}

#[test]
fn non_managed_markdown_query_is_stable_across_index_rebuild() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(workspace.path().join("draft.md"), "# Draft\n\nBody\n").unwrap();
    let before = engine.list_non_managed_markdown().unwrap();

    engine.rebuild_index().unwrap();
    let after = engine.list_non_managed_markdown().unwrap();

    assert_eq!(after, before);
}

#[cfg(unix)]
#[test]
fn workspace_entries_skip_symlinks() {
    use std::os::unix::fs::symlink;

    let (workspace, _app_data, engine) = engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("outside.txt"), "outside").unwrap();
    symlink(outside.path(), workspace.path().join("shortcut")).unwrap();

    let entries = engine.list_workspace_entries().unwrap();

    assert!(
        !entries
            .iter()
            .any(|entry| entry.relative_path == "shortcut")
    );
}

#[test]
fn overlapping_managed_property_edits_conflict_without_writing() {
    let (_workspace, app_data, engine) = engine();
    let created = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Conflict".into(),
            body: "body".into(),
            relative_path: None,
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap();
    let base = created.value;
    let external = WorkspaceEngine::open_with_app_data(engine.root(), app_data.path()).unwrap();
    external
        .update_object(
            &base.id,
            ObjectPatch {
                title: None,
                body: None,
                properties: BTreeMap::from([("status".into(), serde_json::json!("done"))]),
                remove_properties: Vec::new(),
                expected_revision: base.revision.clone(),
            },
        )
        .unwrap();
    let input = ManagedDraftInput {
        id: base.id.clone(),
        base_revision: base.revision.clone(),
        base_title: base.title.clone(),
        base_body: base.body.clone(),
        base_properties: base.properties.clone(),
        local_title: base.title.clone(),
        local_body: base.body.clone(),
        local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
    };
    let result = engine.save_managed_draft(input).unwrap();
    assert!(matches!(result, ManagedDraftResult::Conflict { .. }));
}

#[test]
fn raw_markdown_save_preserves_crlf_and_bom_and_reindexes() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(
        workspace.path().join("scratch.md"),
        b"\xEF\xBB\xBF# Scratch\r\n\r\nlorem\r\n",
    )
    .unwrap();
    engine.reconcile().unwrap();
    let base = engine.read_raw_markdown("scratch.md").unwrap();
    assert!(base.has_bom);
    assert!(base.uses_crlf);
    assert!(base.body.starts_with("# Scratch\n"));
    let saved = engine
        .save_raw_markdown(RawSaveInput {
            relative_path: "scratch.md".into(),
            base_revision: base.revision.clone(),
            base_body: base.body.clone(),
            local_body: "# Scratch\n\nlorem\n\nmore\n".into(),
        })
        .unwrap();
    let RawSaveResult::Saved { current, .. } = saved else {
        panic!("expected saved raw markdown, got {saved:?}");
    };
    assert!(current.uses_crlf);
    assert!(current.has_bom);
    assert_eq!(current.body, "# Scratch\n\nlorem\n\nmore\n");
    let bytes = std::fs::read(workspace.path().join("scratch.md")).unwrap();
    assert_eq!(bytes, b"\xEF\xBB\xBF# Scratch\r\n\r\nlorem\r\n\r\nmore\r\n");
}

#[test]
fn raw_markdown_rejects_traversal_and_non_markdown_paths() {
    let (_workspace, _app_data, engine) = engine();
    assert!(engine.read_raw_markdown("../outside.md").is_err());
    assert!(engine.read_raw_markdown("notes/file.txt").is_err());
    assert!(engine.read_raw_markdown(".noura/history/notes.md").is_err());
}

#[test]
fn markdown_link_fragments_resolve_as_unresolved() {
    let (workspace, _app_data, engine) = engine();
    std::fs::create_dir_all(workspace.path().join("notes")).unwrap();
    std::fs::write(workspace.path().join("notes/target.md"), "# Target\n").unwrap();
    engine.reconcile().unwrap();

    for fragment_only in ["#heading", "|alias", "| alias"] {
        let resolved = engine
            .resolve_markdown_link("notes/source.md", fragment_only)
            .unwrap();
        assert!(
            matches!(resolved, MarkdownLinkTarget::Unresolved),
            "expected `{fragment_only}` to be unresolved, got {resolved:?}"
        );
    }

    // Real files keep resolving with their fragments intact.
    let resolved = engine
        .resolve_markdown_link("notes/source.md", "target#heading")
        .unwrap();
    assert!(matches!(resolved, MarkdownLinkTarget::Markdown { .. }));
}

#[test]
fn markdown_targets_resolve_relative_files_and_reject_workspace_escape() {
    let (workspace, _app_data, engine) = engine();
    std::fs::create_dir_all(workspace.path().join("notes")).unwrap();
    std::fs::write(workspace.path().join("notes/target.md"), "# Target\n").unwrap();
    std::fs::write(workspace.path().join("image.png"), b"png").unwrap();
    engine.reconcile().unwrap();
    let markdown = engine
        .resolve_markdown_link("notes/source.md", "target")
        .unwrap();
    assert!(matches!(markdown, MarkdownLinkTarget::Markdown { .. }));
    let (relative, bytes) = engine
        .read_local_asset("notes/source.md", "../image.png", 1024)
        .unwrap();
    assert_eq!((relative, bytes), ("image.png".into(), b"png".to_vec()));
    assert!(
        engine
            .read_local_asset("notes/source.md", "../../outside.png", 1024)
            .is_err()
    );
}

#[test]
fn raw_markdown_save_reports_managed_identity_after_repair() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(workspace.path().join("repair.md"), "---\nid: broken\n---\n").unwrap();
    engine.reconcile().unwrap();
    let base = engine.read_raw_markdown("repair.md").unwrap();
    let id = new_object_id("note");
    let repaired = format!("---\nid: {id}\ntype: note\n---\n\n# Repaired\n");
    let result = engine
        .save_raw_markdown(RawSaveInput {
            relative_path: "repair.md".into(),
            base_revision: base.revision,
            base_body: base.body,
            local_body: repaired,
        })
        .unwrap();
    let RawSaveResult::Saved { managed_object, .. } = result else {
        panic!("expected repaired raw Markdown to save");
    };
    assert_eq!(managed_object.map(|object| object.id), Some(id));
}

#[test]
fn raw_conflict_resolution_snapshots_each_side() {
    let (workspace, app_data, engine) = engine();
    std::fs::write(workspace.path().join("diary.md"), "day one\n").unwrap();
    engine.reconcile().unwrap();
    let base = engine.read_raw_markdown("diary.md").unwrap();
    let external = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    std::fs::write(
        external.root().join("diary.md"),
        "day one\nday two external\n",
    )
    .unwrap();
    external.reconcile().unwrap();
    let saved = engine
        .save_raw_markdown(RawSaveInput {
            relative_path: "diary.md".into(),
            base_revision: base.revision.clone(),
            base_body: base.body.clone(),
            local_body: "day one\nday two local\n".into(),
        })
        .unwrap();
    let RawSaveResult::Conflict { current, .. } = saved else {
        panic!("expected raw conflict, got {saved:?}");
    };
    let adopted = engine
        .resolve_raw_conflict(RawConflictResolveInput {
            relative_path: "diary.md".into(),
            current_revision: current.revision.clone(),
            local_body: "day one\nday two local\n".into(),
            resolution: ConflictResolution::ReplaceExternal,
        })
        .unwrap();
    assert_eq!(adopted.current.body, "day one\nday two local\n");
    let history = workspace.path().join(".noura/history");
    assert!(history.read_dir().unwrap().next().is_some());
}

#[test]
fn managed_conflict_keeps_current_revisions_until_resolution() {
    let (_workspace, _app_data, engine) = engine();
    let created = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Pinned".into(),
            body: "body".into(),
            relative_path: None,
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap();
    let base = created.value;
    let external =
        WorkspaceEngine::open_with_app_data(engine.root(), tempdir().unwrap().path()).unwrap();
    external
        .update_object(
            &base.id,
            ObjectPatch {
                title: None,
                body: None,
                properties: BTreeMap::from([("status".into(), serde_json::json!("done"))]),
                remove_properties: Vec::new(),
                expected_revision: base.revision.clone(),
            },
        )
        .unwrap();
    let input = ManagedDraftInput {
        id: base.id.clone(),
        base_revision: base.revision.clone(),
        base_title: base.title.clone(),
        base_body: base.body.clone(),
        base_properties: base.properties.clone(),
        local_title: base.title.clone(),
        local_body: base.body.clone(),
        local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
    };
    let result = engine.save_managed_draft(input).unwrap();
    assert!(matches!(result, ManagedDraftResult::Conflict { .. }));
}

#[test]
fn managed_conflict_restore_recreates_an_externally_deleted_file() {
    let (workspace, _app_data, engine) = engine();
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Recover me".into(),
            body: "original body".into(),
            relative_path: Some("tasks/recover-me.md".into()),
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;
    std::fs::remove_file(workspace.path().join(&original.relative_path)).unwrap();
    engine.reconcile().unwrap();

    let restored = engine
        .resolve_managed_conflict(ManagedConflictResolveInput {
            id: original.id.clone(),
            current_revision: original.revision,
            relative_path: original.relative_path.clone(),
            created: original.created.clone(),
            local_title: "Recovered task".into(),
            local_body: "draft survived deletion".into(),
            local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
            resolution: ManagedConflictResolution::ReplaceExternal,
        })
        .unwrap();

    assert_eq!(restored.id, original.id);
    assert_eq!(restored.relative_path, original.relative_path);
    assert_eq!(restored.created, original.created);
    assert_eq!(restored.body, "draft survived deletion");
    assert!(workspace.path().join(&restored.relative_path).is_file());
    assert_eq!(engine.get_object(&restored.id).unwrap(), Some(restored));
}

#[test]
fn managed_conflict_restore_rejects_invalid_created_timestamp() {
    let (workspace, _app_data, engine) = engine();
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Validate my clock".into(),
            body: "original body".into(),
            relative_path: Some("tasks/recover-timestamp.md".into()),
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;
    std::fs::remove_file(workspace.path().join(&original.relative_path)).unwrap();
    engine.reconcile().unwrap();

    let error = engine
        .resolve_managed_conflict(ManagedConflictResolveInput {
            id: original.id,
            current_revision: original.revision,
            relative_path: original.relative_path,
            created: Some("yesterday".into()),
            local_title: "Recovered task".into(),
            local_body: "draft survived deletion".into(),
            local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
            resolution: ManagedConflictResolution::ReplaceExternal,
        })
        .unwrap_err();

    assert_eq!(error.code, "invalid_timestamp");
}

#[test]
fn managed_conflict_restore_rejects_reserved_directories() {
    let (workspace, _app_data, engine) = engine();
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Recover me".into(),
            body: "original body".into(),
            relative_path: Some("tasks/recover-reserved.md".into()),
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;
    std::fs::remove_file(workspace.path().join(&original.relative_path)).unwrap();
    engine.reconcile().unwrap();

    let error = engine
        .resolve_managed_conflict(ManagedConflictResolveInput {
            id: original.id,
            current_revision: original.revision,
            relative_path: ".noura/recovered.md".into(),
            created: original.created,
            local_title: "Recovered task".into(),
            local_body: "draft survived deletion".into(),
            local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
            resolution: ManagedConflictResolution::ReplaceExternal,
        })
        .unwrap_err();

    assert_eq!(error.code, "reserved_path");
}

#[test]
fn managed_conflict_restore_requires_markdown_extension() {
    let (workspace, _app_data, engine) = engine();
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Recover me".into(),
            body: "original body".into(),
            relative_path: Some("tasks/recover-extension.md".into()),
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;
    std::fs::remove_file(workspace.path().join(&original.relative_path)).unwrap();
    engine.reconcile().unwrap();

    let error = engine
        .resolve_managed_conflict(ManagedConflictResolveInput {
            id: original.id,
            current_revision: original.revision,
            relative_path: "tasks/recovered.txt".into(),
            created: original.created,
            local_title: "Recovered task".into(),
            local_body: "draft survived deletion".into(),
            local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
            resolution: ManagedConflictResolution::ReplaceExternal,
        })
        .unwrap_err();

    assert_eq!(error.code, "unsupported_extension");
}

#[test]
fn plugin_state_is_namespaced_per_plugin_and_per_workspace() {
    let (workspace, app_data, engine) = engine();
    engine
        .plugin_state_set("tasks", "view", serde_json::json!("board"))
        .unwrap();
    engine
        .plugin_state_set("calendar", "view", serde_json::json!("week"))
        .unwrap();
    assert_eq!(
        engine.plugin_state_get("tasks", "view").unwrap(),
        Some(serde_json::json!("board"))
    );
    assert_eq!(
        engine.plugin_state_get("calendar", "view").unwrap(),
        Some(serde_json::json!("week"))
    );
    assert_eq!(engine.plugin_state_get("tasks", "missing").unwrap(), None);
    assert!(engine.plugin_state_delete("tasks", "view").unwrap());
    assert_eq!(engine.plugin_state_get("tasks", "view").unwrap(), None);

    // Reopening the same workspace on the same device shares local state.
    let same_device =
        WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    assert_eq!(
        same_device.plugin_state_get("calendar", "view").unwrap(),
        Some(serde_json::json!("week"))
    );

    // A different device keeps its own disposable index.
    let other_device =
        WorkspaceEngine::open_with_app_data(workspace.path(), tempdir().unwrap().path()).unwrap();
    assert_eq!(
        other_device.plugin_state_get("calendar", "view").unwrap(),
        None
    );
}

#[test]
fn plugin_state_rejects_invalid_plugin_ids_and_accepts_hyphenated_ids() {
    let (_workspace, _app_data, engine) = engine();
    for (plugin_id, key) in [
        ("", "view"),
        ("Tasks", "view"),
        ("1tasks", "view"),
        ("tasks with spaces", "view"),
        ("tasks", ""),
        ("tasks", "\n"),
    ] {
        assert!(
            engine
                .plugin_state_set(plugin_id, key, serde_json::json!("x"))
                .is_err(),
            "expected rejection for {plugin_id:?} / {key:?}"
        );
    }
    let key = "key/with:symbols";
    engine
        .plugin_state_set("tasks-v2", key, serde_json::json!({"a": 1}))
        .unwrap();
    assert_eq!(
        engine.plugin_state_get("tasks-v2", key).unwrap(),
        Some(serde_json::json!({"a": 1}))
    );
}

#[test]
fn manifest_update_rewrites_enabled_plugins_durably_and_in_memory() {
    let (workspace, _app_data, engine) = engine();
    let before = engine.read_manifest().unwrap();
    let updated = engine
        .manifest_update(ManifestUpdateInput {
            enabled_plugins: Some(vec!["tasks".into(), "notes".into(), "tasks".into()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        updated.enabled_plugins,
        vec!["notes".to_owned(), "tasks".to_owned()]
    );
    assert_ne!(updated.updated, before.updated);
    assert_eq!(updated.id, before.id);

    let on_disk = std::fs::read_to_string(workspace.path().join("workspace.yaml")).unwrap();
    assert!(on_disk.contains("enabled_plugins:"));
    assert!(on_disk.contains("- notes"));
    assert_eq!(
        engine.read_manifest().unwrap().enabled_plugins,
        updated.enabled_plugins
    );

    let error = engine
        .manifest_update(ManifestUpdateInput {
            enabled_plugins: Some(vec!["calendar".into()]),
            expected_updated: Some(before.updated),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.code, "manifest_conflict");
}

#[test]
fn manifest_update_rejects_invalid_fields_and_leaves_no_op_patches_untouched() {
    let (_workspace, _app_data, engine) = engine();
    let unchanged = engine
        .manifest_update(ManifestUpdateInput::default())
        .unwrap();
    let error = engine
        .manifest_update(ManifestUpdateInput {
            enabled_plugins: Some(vec!["".into()]),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.code, "invalid_plugin_id");
    let error = engine
        .manifest_update(ManifestUpdateInput {
            name: Some("   ".into()),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.code, "workspace_name_required");
    assert_eq!(
        engine.read_manifest().unwrap().enabled_plugins,
        unchanged.enabled_plugins
    );
}

#[test]
fn manifest_read_reflects_external_edits_to_workspace_yaml() {
    let (workspace, _app_data, engine) = engine();
    let bytes = std::fs::read_to_string(workspace.path().join("workspace.yaml")).unwrap();
    assert!(bytes.contains("name: Domain tests"));
    std::fs::write(
        workspace.path().join("workspace.yaml"),
        bytes.replace("name: Domain tests", "name: External name"),
    )
    .unwrap();
    assert_eq!(engine.read_manifest().unwrap().name, "External name");
    // The in-memory snapshot follows the file after a durable update.
    engine
        .manifest_update(ManifestUpdateInput {
            name: Some("Renamed".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(engine.read_manifest().unwrap().name, "Renamed");
}

#[test]
fn pdf_links_resolve_page_fragments_and_encoded_names_without_managing_files() {
    let (root, _data, engine) = engine();
    std::fs::create_dir(root.path().join("course")).unwrap();
    std::fs::write(
        root.path().join("course/lecture notes.pdf"),
        b"%PDF-1.7\nfixture",
    )
    .unwrap();
    for (target, expected) in [
        ("lecture%20notes.pdf#page=7", Some(7)),
        ("lecture notes.pdf#page=0", None),
        ("lecture notes.pdf#page=oops|Read", None),
    ] {
        let resolved = engine
            .resolve_markdown_link("course/note.md", target)
            .unwrap();
        match resolved {
            MarkdownLinkTarget::Pdf {
                relative_path,
                page,
            } => {
                assert_eq!(relative_path, "course/lecture notes.pdf");
                assert_eq!(page, expected);
            }
            _ => panic!("expected PDF target"),
        }
    }
    assert!(
        engine
            .resolve_markdown_link("course/note.md", "../../outside.pdf")
            .is_err()
    );
    assert!(
        engine
            .resolve_markdown_link("course/note.md", "%2e%2e/%2e%2e/outside.pdf")
            .is_err()
    );
    let read = |engine: &WorkspaceEngine, info: &local_core::PdfInfo| {
        engine
            .read_pdf_range(&local_core::PdfRangeInput {
                relative_path: info.relative_path.clone(),
                workspace_id: info.workspace_id.clone(),
                version: info.version.clone(),
                offset: 0,
                length: info.length,
            })
            .unwrap()
    };
    let first = engine.inspect_pdf("course/lecture notes.pdf").unwrap();
    let bytes = read(&engine, &first);
    engine.rebuild_index().unwrap();
    assert_eq!(
        first.version,
        engine
            .inspect_pdf("course/lecture notes.pdf")
            .unwrap()
            .version
    );
    assert_eq!(bytes, read(&engine, &first));
    std::fs::rename(
        root.path().join("course/lecture notes.pdf"),
        root.path().join("course/moved.pdf"),
    )
    .unwrap();
    assert!(engine.inspect_pdf("course/lecture notes.pdf").is_err());
    assert_eq!(
        bytes,
        read(&engine, &engine.inspect_pdf("course/moved.pdf").unwrap())
    );
    assert_eq!(
        first.length,
        engine.inspect_pdf("course/moved.pdf").unwrap().length
    );
}
