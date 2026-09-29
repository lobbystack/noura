//! Chats and their messages.

use local_core::{
    AppendChatContextSummaryInput, AppendChatToolResultInput, AppendChatUserMessageInput,
    BeginChatAssistantInput, BeginChatToolCallInput, ChangeChatRetentionInput, Chat, ChatMessage,
    ChatRead, CoreError, CreateChatInput, FinishChatAssistantInput, FinishChatToolCallInput,
    MutationResult, RenameChatInput, WorkspaceEngine,
};
use tauri::State;

use crate::{AppState, with_engine};

#[tauri::command(async)]
pub fn chats_create(
    state: State<AppState>,
    input: CreateChatInput,
) -> Result<MutationResult<Chat>, CoreError> {
    with_engine(&state, "chat_create", |engine| engine.create_chat(input))
}

#[tauri::command(async)]
pub fn chats_change_retention(
    state: State<AppState>,
    input: ChangeChatRetentionInput,
) -> Result<MutationResult<Chat>, CoreError> {
    with_engine(&state, "chat_change_retention", |engine| {
        engine.change_chat_retention(input)
    })
}

#[tauri::command(async)]
pub fn chats_rename(
    state: State<AppState>,
    input: RenameChatInput,
) -> Result<MutationResult<Chat>, CoreError> {
    with_engine(&state, "chat_rename", |engine| engine.rename_chat(input))
}

#[tauri::command(async)]
pub fn chats_list(state: State<AppState>) -> Result<Vec<Chat>, CoreError> {
    with_engine(&state, "chat_list", WorkspaceEngine::list_chats)
}

#[tauri::command(async)]
pub fn chats_read(state: State<AppState>, id: String) -> Result<ChatRead, CoreError> {
    with_engine(&state, "chat_read", |engine| engine.read_chat(&id))
}

#[tauri::command(async)]
pub fn chats_append_user_message(
    state: State<AppState>,
    input: AppendChatUserMessageInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_append_user_message", |engine| {
        engine.append_chat_user_message(input)
    })
}

#[tauri::command(async)]
pub fn chats_begin_assistant(
    state: State<AppState>,
    input: BeginChatAssistantInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_begin_assistant", |engine| {
        engine.begin_chat_assistant(input)
    })
}

#[tauri::command(async)]
pub fn chats_finish_assistant(
    state: State<AppState>,
    input: FinishChatAssistantInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_finish_assistant", |engine| {
        engine.finish_chat_assistant(input)
    })
}

#[tauri::command(async)]
pub fn chats_begin_tool_call(
    state: State<AppState>,
    input: BeginChatToolCallInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_begin_tool_call", |engine| {
        engine.begin_chat_tool_call(input)
    })
}

#[tauri::command(async)]
pub fn chats_finish_tool_call(
    state: State<AppState>,
    input: FinishChatToolCallInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_finish_tool_call", |engine| {
        engine.finish_chat_tool_call(input)
    })
}

#[tauri::command(async)]
pub fn chats_append_tool_result(
    state: State<AppState>,
    input: AppendChatToolResultInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_append_tool_result", |engine| {
        engine.append_chat_tool_result(input)
    })
}

#[tauri::command(async)]
pub fn chats_append_context_summary(
    state: State<AppState>,
    input: AppendChatContextSummaryInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_append_context_summary", |engine| {
        engine.append_chat_context_summary(input)
    })
}

#[tauri::command(async)]
pub fn chats_recover_interrupted(
    state: State<AppState>,
    id: String,
) -> Result<ChatRead, CoreError> {
    with_engine(&state, "chat_recover_interrupted", |engine| {
        engine.recover_interrupted_chat(&id)
    })
}

#[tauri::command(async)]
pub fn chats_expire(state: State<AppState>, now: String) -> Result<Vec<String>, CoreError> {
    with_engine(&state, "chat_expire", |engine| engine.expire_chats(&now))
}
