//! Chats and their messages, stored as Markdown files under the workspace.

use super::*;

impl WorkspaceEngine {
    pub fn create_chat(&self, input: CreateChatInput) -> Result<MutationResult<Chat>> {
        let title = validate_chat_title(&input.title, "chat_create")?;
        let retention = input.retention.unwrap_or(ChatRetention::Permanent);
        validate_retention(retention, input.retention_days, "chat_create")?;
        let now = now_rfc3339();
        let id = new_object_id("chat");
        let slug = slug::slugify(title);
        let slug = if slug.is_empty() { "untitled" } else { &slug };
        let short_id = id[id.len() - 6..].to_owned();
        let mut chat = Chat {
            id,
            title: title.into(),
            relative_path: format!("chats/{slug}--{short_id}/chat.md"),
            revision: String::new(),
            created: now.clone(),
            updated: now,
            retention,
            retention_days: input.retention_days,
            properties: BTreeMap::new(),
        };
        let _guard = self.write_lock("chat_create")?;
        let (index_status, warnings) = self.write_chat_unlocked(&mut chat, None, "chat_create")?;
        self.emit_chat_event(
            "chat:created",
            &chat,
            EventSource::Application,
            serde_json::json!({}),
        );
        Ok(MutationResult {
            value: chat.clone(),
            revision: chat.revision,
            durability: "committed".into(),
            index_status,
            warnings,
            chat_revision: None,
        })
    }

    pub fn change_chat_retention(
        &self,
        input: ChangeChatRetentionInput,
    ) -> Result<MutationResult<Chat>> {
        validate_retention(
            input.retention,
            input.retention_days,
            "chat_change_retention",
        )?;
        let mut chat = self.read_canonical_chat(&input.chat_id, "chat_change_retention")?;
        let _guard = self.write_lock("chat_change_retention")?;
        self.check_chat_revision_unlocked(
            &chat,
            &input.expected_chat_revision,
            "chat_change_retention",
        )?;
        chat.retention = input.retention;
        chat.retention_days = input.retention_days;
        chat.updated = now_rfc3339();
        let (index_status, warnings) = self.write_chat_unlocked(
            &mut chat,
            Some(&input.expected_chat_revision),
            "chat_change_retention",
        )?;
        self.emit_chat_event(
            "chat:retention-changed",
            &chat,
            EventSource::Application,
            serde_json::json!({}),
        );
        Ok(MutationResult {
            value: chat.clone(),
            revision: chat.revision,
            durability: "committed".into(),
            index_status,
            warnings,
            chat_revision: None,
        })
    }

    pub fn rename_chat(&self, input: RenameChatInput) -> Result<MutationResult<Chat>> {
        let title = validate_chat_title(&input.title, "chat_rename")?;
        let mut chat = self.read_canonical_chat(&input.chat_id, "chat_rename")?;
        let _guard = self.write_lock("chat_rename")?;
        self.check_chat_revision_unlocked(&chat, &input.expected_chat_revision, "chat_rename")?;
        chat.title = title.into();
        chat.updated = now_rfc3339();
        let (index_status, warnings) = self.write_chat_unlocked(
            &mut chat,
            Some(&input.expected_chat_revision),
            "chat_rename",
        )?;
        self.emit_chat_event(
            "chat:renamed",
            &chat,
            EventSource::Application,
            serde_json::json!({}),
        );
        Ok(MutationResult {
            value: chat.clone(),
            revision: chat.revision,
            durability: "committed".into(),
            index_status,
            warnings,
            chat_revision: None,
        })
    }

    pub fn list_chats(&self) -> Result<Vec<Chat>> {
        self.reconcile_if_needed()?;
        let objects = self.query_objects(Some("chat"))?;
        let mut chats = Vec::with_capacity(objects.len());
        for object in objects {
            let path = resolve_for_write(&self.root, &object.relative_path, "chat_list")?;
            let bytes = std::fs::read(&path)
                .map_err(|error| CoreError::io(error, "chat_list", Some(&object.relative_path)))?;
            let chat = parse_chat(&object.relative_path, &bytes)?;
            chats.push((parse_chat_timestamp(&chat.updated, "chat_list")?, chat));
        }
        chats.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
        Ok(chats.into_iter().map(|(_, chat)| chat).collect())
    }

    pub fn read_chat(&self, id: &str) -> Result<ChatRead> {
        let chat = self.read_canonical_chat(id, "chat_read")?;
        let messages = match self.read_chat_messages(id) {
            Ok(messages) => messages,
            // The index pointed at a message file that moved or vanished
            // before the watcher reported it: walk once and read again.
            Err(error) if error.code == "filesystem_error" => {
                self.reconcile()?;
                self.read_chat_messages(id)?
            }
            Err(error) => return Err(error),
        };
        Ok(ChatRead { chat, messages })
    }

    /// Messages of one chat, found through the index's `chat_id` relation and
    /// read from their files, oldest first.
    fn read_chat_messages(&self, id: &str) -> Result<Vec<ChatMessage>> {
        let paths = self
            .index
            .lock()
            .map_err(|_| lock_error("chat_read"))?
            .chat_message_paths(id)?;
        let mut messages = Vec::with_capacity(paths.len());
        for relative in paths {
            let path = resolve_for_write(&self.root, &relative, "chat_read")?;
            let bytes = std::fs::read(path)
                .map_err(|error| CoreError::io(error, "chat_read", Some(&relative)))?;
            let message = parse_chat_message(&relative, &bytes)?;
            if message.chat_id != id {
                continue;
            }
            messages.push((
                parse_chat_timestamp(&message.created, "chat_read")?,
                message,
            ));
        }
        messages.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
        Ok(messages.into_iter().map(|(_, message)| message).collect())
    }

    pub fn append_chat_user_message(
        &self,
        input: AppendChatUserMessageInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::User,
            ChatMessageStatus::Completed,
            input.content,
            input.run_id,
            None,
            None,
            None,
            None,
            None,
            "chat:message-appended",
            "chat_append_user_message",
        )
    }

    pub fn begin_chat_assistant(
        &self,
        input: BeginChatAssistantInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::Assistant,
            ChatMessageStatus::InProgress,
            String::new(),
            input.run_id,
            Some(input.provider_id),
            Some(input.model_id),
            None,
            None,
            None,
            "chat:assistant-began",
            "chat_begin_assistant",
        )
    }

    pub fn finish_chat_assistant(
        &self,
        input: FinishChatAssistantInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.finish_chat_message(
            &input.chat_id,
            &input.message_id,
            &input.expected_chat_revision,
            &input.expected_message_revision,
            ChatMessageKind::Assistant,
            input.content,
            input.status,
            input.error_code,
            "chat:assistant-finished",
            "chat_finish_assistant",
        )
    }

    pub fn begin_chat_tool_call(
        &self,
        input: BeginChatToolCallInput,
    ) -> Result<MutationResult<ChatMessage>> {
        validate_tool_fields(
            &input.tool_call_id,
            &input.tool_name,
            "chat_begin_tool_call",
        )?;
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::ToolCall,
            ChatMessageStatus::InProgress,
            input.content,
            input.run_id,
            None,
            None,
            Some(input.tool_call_id),
            Some(input.tool_name),
            None,
            "chat:tool-call-began",
            "chat_begin_tool_call",
        )
    }

    pub fn finish_chat_tool_call(
        &self,
        input: FinishChatToolCallInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.finish_chat_message(
            &input.chat_id,
            &input.message_id,
            &input.expected_chat_revision,
            &input.expected_message_revision,
            ChatMessageKind::ToolCall,
            input.content,
            input.status,
            input.error_code,
            "chat:tool-call-finished",
            "chat_finish_tool_call",
        )
    }

    pub fn append_chat_tool_result(
        &self,
        input: AppendChatToolResultInput,
    ) -> Result<MutationResult<ChatMessage>> {
        validate_tool_fields(
            &input.tool_call_id,
            &input.tool_name,
            "chat_append_tool_result",
        )?;
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::ToolResult,
            ChatMessageStatus::Completed,
            input.content,
            input.run_id,
            None,
            None,
            Some(input.tool_call_id),
            Some(input.tool_name),
            None,
            "chat:tool-result-appended",
            "chat_append_tool_result",
        )
    }

    pub fn append_chat_context_summary(
        &self,
        input: AppendChatContextSummaryInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::ContextSummary,
            ChatMessageStatus::Completed,
            input.content,
            input.run_id,
            None,
            None,
            None,
            None,
            Some(input.summarizes_through_message_id),
            "chat:context-summary-appended",
            "chat_append_context_summary",
        )
    }

    pub fn recover_interrupted_chat(&self, id: &str) -> Result<ChatRead> {
        let mut read = self.read_chat(id)?;
        let completed_tool_results = read
            .messages
            .iter()
            .filter(|message| {
                message.kind == ChatMessageKind::ToolResult
                    && message.status == ChatMessageStatus::Completed
            })
            .cloned()
            .collect::<Vec<_>>();
        let interrupted = read
            .messages
            .iter_mut()
            .filter(|message| message.status == ChatMessageStatus::InProgress)
            .collect::<Vec<_>>();
        if interrupted.is_empty() {
            return Ok(read);
        }
        let _guard = self.write_lock("chat_recover_interrupted")?;
        let mut recovered_count = 0;
        for message in interrupted {
            // The write lock is already held. Reconciliation here would try to
            // acquire it again while recovering a pending mutation, so read the
            // canonical path captured in the preceding reconciled ChatRead.
            let path = resolve_for_write(
                &self.root,
                &message.relative_path,
                "chat_recover_interrupted",
            )?;
            let bytes = std::fs::read(&path).map_err(|error| {
                CoreError::io(
                    error,
                    "chat_recover_interrupted",
                    Some(&message.relative_path),
                )
            })?;
            let current = parse_chat_message(&message.relative_path, &bytes)?;
            if current.status != ChatMessageStatus::InProgress {
                *message = current;
                continue;
            }
            let mut recovered = current;
            recovered.status = if completed_tool_results
                .iter()
                .any(|result| tool_result_completes_call(result, &recovered))
            {
                ChatMessageStatus::Completed
            } else {
                ChatMessageStatus::Interrupted
            };
            recovered.updated = now_rfc3339();
            let expected_chat_revision = read.chat.revision.clone();
            read.chat.updated = now_rfc3339();
            self.commit_chat_mutation_unlocked(
                &mut read.chat,
                Some(&expected_chat_revision),
                &mut recovered,
                Some(&message.revision),
                "chat_recover_interrupted",
            )?;
            *message = recovered;
            recovered_count += 1;
        }
        if recovered_count == 0 {
            return Ok(read);
        }
        self.emit_chat_event(
            "chat:recovered",
            &read.chat,
            EventSource::Application,
            serde_json::json!({ "interruptedMessages": read.messages.iter().filter(|message| message.status == ChatMessageStatus::Interrupted).count() }),
        );
        Ok(read)
    }

    pub fn expire_chats(&self, now: &str) -> Result<Vec<String>> {
        let now = now.parse::<jiff::Timestamp>().map_err(|_| {
            CoreError::validation(
                "invalid_timestamp",
                "Expiry time must be an RFC 3339 timestamp",
                "chat_expire",
            )
        })?;
        let chats = self.list_chats()?;
        let _guard = self.write_lock("chat_expire")?;
        let mut expired = Vec::new();
        for chat in chats {
            let Some(days) = chat.retention_days else {
                continue;
            };
            let created = chat.created.parse::<jiff::Timestamp>().map_err(|_| {
                CoreError::validation(
                    "invalid_timestamp",
                    "Chat creation time must be an RFC 3339 timestamp",
                    "chat_expire",
                )
            })?;
            let expires_at = created
                .checked_add(jiff::SignedDuration::from_hours(i64::from(days) * 24))
                .map_err(|_| {
                    CoreError::validation(
                        "invalid_timestamp",
                        "Chat retention period is invalid",
                        "chat_expire",
                    )
                })?;
            if expires_at > now || !self.expire_chat_directory(&chat, expires_at)? {
                continue;
            }
            expired.push(chat.id.clone());
            self.emit_chat_event(
                "chat:expired",
                &chat,
                EventSource::Application,
                serde_json::json!({}),
            );
        }
        drop(_guard);
        if !expired.is_empty() {
            self.reconcile()?;
        }
        Ok(expired)
    }

    fn read_canonical_chat(&self, id: &str, operation: &str) -> Result<Chat> {
        if !valid_object_id(id, "chat") {
            return Err(CoreError::validation(
                "invalid_chat_id",
                "The chat ID is invalid",
                operation,
            ));
        }
        let Some((object, bytes)) = self
            .read_canonical_file(id, operation)?
            .filter(|(object, _)| object.object_type == "chat")
        else {
            return Err(CoreError::validation(
                "chat_not_found",
                "The chat does not exist",
                operation,
            ));
        };
        parse_chat(&object.relative_path, &bytes)
    }

    fn read_canonical_chat_message(&self, id: &str, operation: &str) -> Result<ChatMessage> {
        if !valid_object_id(id, "chat-message") {
            return Err(CoreError::validation(
                "invalid_chat_message_id",
                "The chat message ID is invalid",
                operation,
            ));
        }
        let Some((object, bytes)) = self
            .read_canonical_file(id, operation)?
            .filter(|(object, _)| object.object_type == "chat-message")
        else {
            return Err(CoreError::validation(
                "chat_message_not_found",
                "The chat message does not exist",
                operation,
            ));
        };
        parse_chat_message(&object.relative_path, &bytes)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The durable message append contract carries all canonical message fields without a second transient input type"
    )]
    fn append_chat_message(
        &self,
        chat_id: &str,
        expected_chat_revision: &str,
        kind: ChatMessageKind,
        status: ChatMessageStatus,
        mut content: String,
        run_id: String,
        provider_id: Option<String>,
        model_id: Option<String>,
        tool_call_id: Option<String>,
        tool_name: Option<String>,
        summarizes_through_message_id: Option<String>,
        event: &str,
        operation: &str,
    ) -> Result<MutationResult<ChatMessage>> {
        if matches!(
            &kind,
            ChatMessageKind::ToolCall | ChatMessageKind::ToolResult
        ) {
            content = crate::chat::canonical_json(&content, operation)?;
        }
        let mut chat = self.read_canonical_chat(chat_id, operation)?;
        if let Some(summary_id) = &summarizes_through_message_id {
            let summarized = self.read_canonical_chat_message(summary_id, operation)?;
            if summarized.chat_id != chat.id {
                return Err(CoreError::validation(
                    "chat_message_mismatch",
                    "The summarized message belongs to another chat",
                    operation,
                ));
            }
        }
        let now = now_rfc3339();
        let id = new_object_id("chat-message");
        let date = now.get(..10).ok_or_else(|| {
            CoreError::validation(
                "invalid_timestamp",
                "The current timestamp is invalid",
                operation,
            )
        })?;
        let parent = Path::new(&chat.relative_path).parent().ok_or_else(|| {
            CoreError::validation("invalid_chat_path", "The chat path is invalid", operation)
        })?;
        let relative_path = parent
            .join("messages")
            .join(date)
            .join(format!("{id}.md"))
            .to_str()
            .map(|value| value.replace(std::path::MAIN_SEPARATOR, "/"))
            .ok_or_else(|| {
                CoreError::validation(
                    "non_utf8_path",
                    "The chat message path is not UTF-8",
                    operation,
                )
            })?;
        let content_type = if matches!(
            &kind,
            ChatMessageKind::ToolCall | ChatMessageKind::ToolResult
        ) {
            "application/json".into()
        } else {
            "text/markdown".into()
        };
        let mut message = ChatMessage {
            id,
            chat_id: chat_id.into(),
            run_id,
            kind,
            status,
            content_type,
            content,
            relative_path,
            revision: String::new(),
            created: now.clone(),
            updated: now.clone(),
            provider_id,
            model_id,
            tool_call_id,
            tool_name,
            error_code: None,
            summarizes_through_message_id,
            properties: BTreeMap::new(),
        };
        crate::chat::validate_message_shape(&message, operation)?;
        let _guard = self.write_lock(operation)?;
        self.check_chat_revision_unlocked(&chat, expected_chat_revision, operation)?;
        chat.updated = now;
        let (chat_index_status, message_index_status, warnings) = self
            .commit_chat_mutation_unlocked(
                &mut chat,
                Some(expected_chat_revision),
                &mut message,
                None,
                operation,
            )?;
        let index_status = if matches!(message_index_status, IndexStatus::RepairPending)
            || matches!(chat_index_status, IndexStatus::RepairPending)
        {
            IndexStatus::RepairPending
        } else {
            IndexStatus::Updated
        };
        self.emit_chat_event(
            event,
            &chat,
            EventSource::Application,
            serde_json::json!({
                "messageId": message.id,
                "messagePath": message.relative_path,
                "messageRevision": message.revision,
            }),
        );
        Ok(MutationResult {
            value: message.clone(),
            revision: message.revision,
            durability: "committed".into(),
            index_status,
            warnings,
            chat_revision: Some(chat.revision),
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The durable message finish contract checks both canonical revisions and preserves explicit lifecycle context"
    )]
    fn finish_chat_message(
        &self,
        chat_id: &str,
        message_id: &str,
        expected_chat_revision: &str,
        expected_message_revision: &str,
        expected_kind: ChatMessageKind,
        content: String,
        status: ChatMessageStatus,
        error_code: Option<String>,
        event: &str,
        operation: &str,
    ) -> Result<MutationResult<ChatMessage>> {
        let mut chat = self.read_canonical_chat(chat_id, operation)?;
        let mut message = self.read_canonical_chat_message(message_id, operation)?;
        if message.chat_id != chat.id || message.kind != expected_kind {
            return Err(CoreError::validation(
                "chat_message_mismatch",
                "The chat message does not match this operation",
                operation,
            ));
        }
        if message.status != ChatMessageStatus::InProgress {
            return Err(CoreError::validation(
                "chat_message_not_in_progress",
                "Only in-progress chat messages can be finished",
                operation,
            ));
        }
        if !matches!(
            &status,
            ChatMessageStatus::Completed
                | ChatMessageStatus::Cancelled
                | ChatMessageStatus::Failed
                | ChatMessageStatus::Interrupted
        ) {
            return Err(CoreError::validation(
                "chat_message_not_terminal",
                "Finished chat messages require a terminal status",
                operation,
            ));
        }
        message.content = if matches!(
            &message.kind,
            ChatMessageKind::ToolCall | ChatMessageKind::ToolResult
        ) {
            crate::chat::canonical_json(&content, operation)?
        } else {
            content
        };
        message.status = status;
        message.error_code = error_code;
        crate::chat::validate_message_shape(&message, operation)?;
        let _guard = self.write_lock(operation)?;
        self.check_chat_revision_unlocked(&chat, expected_chat_revision, operation)?;
        self.check_chat_message_revision_unlocked(&message, expected_message_revision, operation)?;
        chat.updated = now_rfc3339();
        message.updated = now_rfc3339();
        let (chat_index_status, message_index_status, warnings) = self
            .commit_chat_mutation_unlocked(
                &mut chat,
                Some(expected_chat_revision),
                &mut message,
                Some(expected_message_revision),
                operation,
            )?;
        let index_status = if matches!(message_index_status, IndexStatus::RepairPending)
            || matches!(chat_index_status, IndexStatus::RepairPending)
        {
            IndexStatus::RepairPending
        } else {
            IndexStatus::Updated
        };
        self.emit_chat_event(
            event,
            &chat,
            EventSource::Application,
            serde_json::json!({
                "messageId": message.id,
                "messagePath": message.relative_path,
                "messageRevision": message.revision,
            }),
        );
        Ok(MutationResult {
            value: message.clone(),
            revision: message.revision,
            durability: "committed".into(),
            index_status,
            warnings,
            chat_revision: Some(chat.revision),
        })
    }

    fn write_chat_unlocked(
        &self,
        chat: &mut Chat,
        expected_revision: Option<&str>,
        operation: &str,
    ) -> Result<(IndexStatus, Vec<CoreWarning>)> {
        let relative = crate::path::validate_relative(&chat.relative_path, operation)?;
        let destination = resolve_for_write(&self.root, &chat.relative_path, operation)?;
        if let Some(expected) = expected_revision {
            let current = std::fs::read(&destination)
                .map_err(|error| CoreError::io(error, operation, Some(&chat.relative_path)))?;
            check_revision(&current, expected, operation)?;
        } else if destination.exists() {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the requested path",
                operation,
            ));
        }
        let bytes = serialize_chat(chat)?;
        let revision = markdown::revision(&bytes);
        atomic_write_checked(&self.root, &relative, &bytes, expected_revision, operation)?;
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(chat.relative_path.clone(), revision.clone());
        }
        chat.revision = revision;
        let parsed = ParsedMarkdown::Managed(chat.workspace_object());
        let result = self
            .index
            .lock()
            .map_err(|_| lock_error(operation))
            .and_then(|mut index| {
                index.upsert_markdown(&chat.relative_path, &bytes, mtime_ns(&destination), &parsed)
            });
        Ok(self.index_outcome(result))
    }

    fn check_chat_revision_unlocked(
        &self,
        chat: &Chat,
        expected_revision: &str,
        operation: &str,
    ) -> Result<()> {
        let path = resolve_for_write(&self.root, &chat.relative_path, operation)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, operation, Some(&chat.relative_path)))?;
        check_revision(&bytes, expected_revision, operation)
    }

    fn check_chat_message_revision_unlocked(
        &self,
        message: &ChatMessage,
        expected_revision: &str,
        operation: &str,
    ) -> Result<()> {
        let path = resolve_for_write(&self.root, &message.relative_path, operation)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, operation, Some(&message.relative_path)))?;
        check_revision(&bytes, expected_revision, operation)
    }

    fn expire_chat_directory(&self, chat: &Chat, expires_at: jiff::Timestamp) -> Result<bool> {
        let relative = Path::new(&chat.relative_path);
        let Some(directory) = relative.parent() else {
            return Ok(false);
        };
        if relative.file_name().and_then(|value| value.to_str()) != Some("chat.md")
            || directory
                .parent()
                .and_then(|value| value.file_name())
                .and_then(|value| value.to_str())
                != Some("chats")
        {
            return Ok(false);
        }
        let source = resolve_for_write(&self.root, &directory.to_string_lossy(), "chat_expire")?;
        let mut files = Vec::new();
        if collect_expiring_chat_files(&source, &mut files, "chat_expire").is_err() {
            return Ok(false);
        }
        if files.is_empty() || !files.iter().any(|path| path == &source.join("chat.md")) {
            return Ok(false);
        }
        for path in &files {
            let relative = match path.strip_prefix(&self.root).ok().and_then(Path::to_str) {
                Some(relative) => relative.replace('\\', "/"),
                None => return Ok(false),
            };
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(_) => return Ok(false),
            };
            if path.file_name().and_then(|value| value.to_str()) == Some("chat.md") {
                if parse_chat(&relative, &bytes)
                    .ok()
                    .as_ref()
                    .map(|value| &value.id)
                    != Some(&chat.id)
                {
                    return Ok(false);
                }
            } else {
                let Some(message) = parse_chat_message(&relative, &bytes).ok() else {
                    return Ok(false);
                };
                if message.chat_id != chat.id
                    || path.file_name().and_then(|value| value.to_str())
                        != Some(format!("{}.md", message.id).as_str())
                {
                    return Ok(false);
                }
            }
            let modified = path
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| {
                    jiff::Timestamp::new(duration.as_secs() as i64, duration.subsec_nanos() as i32)
                })
                .transpose()
                .map_err(|_| {
                    CoreError::validation(
                        "invalid_timestamp",
                        "A chat file timestamp is invalid",
                        "chat_expire",
                    )
                })?;
            if modified.is_some_and(|modified| modified > expires_at) {
                return Ok(false);
            }
        }
        match self.discard(&source, &directory.to_string_lossy(), "chat_expire") {
            Ok(_) => {}
            Err(error) if error.code == "trash_destination_exists" => return Ok(false),
            Err(error) => return Err(error),
        }
        if let Ok(mut journal) = self.self_writes.lock() {
            for file in files {
                if let Some(relative) = file.strip_prefix(&self.root).ok().and_then(Path::to_str) {
                    journal.insert(relative.replace('\\', "/"), "<deleted>".into());
                }
            }
        }
        Ok(true)
    }

    fn emit_chat_event(
        &self,
        event_type: &str,
        chat: &Chat,
        source: EventSource,
        payload: serde_json::Value,
    ) {
        self.emit(
            event_type,
            source,
            serde_json::json!({
                "id": chat.id,
                "path": chat.relative_path,
                "revision": chat.revision,
                "payload": payload,
            }),
        );
    }
}

fn validate_tool_fields(tool_call_id: &str, tool_name: &str, operation: &str) -> Result<()> {
    if tool_call_id.trim().is_empty()
        || tool_name.trim().is_empty()
        || tool_call_id.chars().any(char::is_control)
        || tool_name.chars().any(char::is_control)
    {
        return Err(CoreError::validation(
            "invalid_tool_message",
            "Tool messages require a tool call ID and tool name",
            operation,
        ));
    }
    Ok(())
}

fn parse_chat_timestamp(value: &str, operation: &str) -> Result<jiff::Timestamp> {
    value.parse::<jiff::Timestamp>().map_err(|_| {
        CoreError::validation(
            "invalid_timestamp",
            "Chat timestamps must be RFC 3339 timestamps",
            operation,
        )
    })
}

fn tool_result_completes_call(result: &ChatMessage, call: &ChatMessage) -> bool {
    call.kind == ChatMessageKind::ToolCall
        && result.run_id == call.run_id
        && result.tool_call_id == call.tool_call_id
        && result.tool_name == call.tool_name
}

/// Retention only moves a directory when it contains the exact canonical chat
/// layout. Unknown files, symlinks, malformed messages, and hand-created
/// folders make the chat ineligible rather than risking unrelated data.
fn collect_expiring_chat_files(
    path: &Path,
    files: &mut Vec<PathBuf>,
    operation: &str,
) -> Result<()> {
    let entries =
        std::fs::read_dir(path).map_err(|error| CoreError::io(error, operation, path.to_str()))?;
    for entry in entries {
        let entry = entry.map_err(|error| CoreError::io(error, operation, path.to_str()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| CoreError::io(error, operation, entry.path().to_str()))?;
        let child = entry.path();
        if file_type.is_symlink() {
            return Err(CoreError::validation(
                "symlink_escape",
                "Chat retention cannot traverse symlinks",
                operation,
            ));
        }
        if file_type.is_file() {
            if child.file_name().and_then(|value| value.to_str()) == Some("chat.md")
                || child
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.starts_with("chat-message_") && name.ends_with(".md"))
            {
                files.push(child);
                continue;
            }
            return Err(CoreError::validation(
                "unsafe_chat_expiry",
                "Chat retention found an unexpected file",
                operation,
            ));
        }
        if file_type.is_dir() {
            let name = child
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if name == "messages"
                || (name.len() == 10
                    && name.as_bytes().get(4) == Some(&b'-')
                    && name.as_bytes().get(7) == Some(&b'-'))
            {
                collect_expiring_chat_files(&child, files, operation)?;
                continue;
            }
        }
        return Err(CoreError::validation(
            "unsafe_chat_expiry",
            "Chat retention found an unexpected path",
            operation,
        ));
    }
    Ok(())
}

fn validate_chat_title<'a>(title: &'a str, operation: &str) -> Result<&'a str> {
    let trimmed = title.trim();
    if trimmed.is_empty() || title.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "chat_title_required",
            "A chat title without control characters is required",
            operation,
        ));
    }
    Ok(trimmed)
}
