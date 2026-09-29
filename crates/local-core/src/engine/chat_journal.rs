//! The chat mutation journal. A chat step writes two files, so the engine
//! records its intent first and replays it after a crash.

use super::*;

const CHAT_MUTATION_DIR: &str = ".noura/chat-mutations";

#[derive(Debug, Serialize, Deserialize)]
struct ChatMutationIntent {
    version: u8,
    chat: ChatMutationTarget,
    message: ChatMutationTarget,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMutationTarget {
    relative_path: String,
    expected_revision: Option<String>,
    bytes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatMutationWriteTarget {
    Chat,
    Message,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ChatMutationFault {
    target: ChatMutationWriteTarget,
    remaining: usize,
}

impl WorkspaceEngine {
    /// Test hook for exercising recovery after either replacement in the
    /// multi-file chat commit. The fault is consumed before the selected write.
    #[doc(hidden)]
    pub fn fail_chat_mutation_write_for_testing(&self, target: &str, times: usize) {
        let target = match target {
            "chat" => ChatMutationWriteTarget::Chat,
            "message" => ChatMutationWriteTarget::Message,
            _ => panic!("unknown chat mutation write target: {target}"),
        };
        *self
            .chat_mutation_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            (times > 0).then_some(ChatMutationFault {
                target,
                remaining: times,
            });
    }

    /// Stage both canonical payloads in a durable workspace intent before
    /// replacing either file. A crash or write error leaves the intent as the
    /// sole authority for replaying the other replacement without guessing.
    pub(super) fn commit_chat_mutation_unlocked(
        &self,
        chat: &mut Chat,
        expected_chat_revision: Option<&str>,
        message: &mut ChatMessage,
        expected_message_revision: Option<&str>,
        operation: &str,
    ) -> Result<(IndexStatus, IndexStatus, Vec<CoreWarning>)> {
        let chat_bytes = serialize_chat(chat)?;
        let message_bytes = serialize_chat_message(message)?;
        let intent = ChatMutationIntent {
            version: 1,
            chat: ChatMutationTarget {
                relative_path: chat.relative_path.clone(),
                expected_revision: expected_chat_revision.map(str::to_owned),
                bytes: String::from_utf8(chat_bytes.clone()).map_err(|_| {
                    CoreError::new(
                        "chat_mutation_serialize_failed",
                        ErrorCategory::Parse,
                        "Chat Markdown could not be staged for commit",
                        operation,
                    )
                })?,
            },
            message: ChatMutationTarget {
                relative_path: message.relative_path.clone(),
                expected_revision: expected_message_revision.map(str::to_owned),
                bytes: String::from_utf8(message_bytes.clone()).map_err(|_| {
                    CoreError::new(
                        "chat_mutation_serialize_failed",
                        ErrorCategory::Parse,
                        "Chat message Markdown could not be staged for commit",
                        operation,
                    )
                })?,
            },
        };
        self.validate_chat_mutation_intent(&intent, operation)?;
        let intent_path = self.write_chat_mutation_intent(&intent, operation)?;
        self.journal_chat_mutation(&intent);

        // Write the child first. Neither replacement is a successful mutation
        // until recovery has observed both recorded target revisions.
        self.apply_chat_mutation_target(
            &intent.message,
            ChatMutationWriteTarget::Message,
            true,
            operation,
        )?;
        self.apply_chat_mutation_target(
            &intent.chat,
            ChatMutationWriteTarget::Chat,
            true,
            operation,
        )?;
        let _ = self.clear_chat_mutation_intent(&intent_path, operation);

        chat.revision = markdown::revision(&chat_bytes);
        message.revision = markdown::revision(&message_bytes);
        let chat_parsed = ParsedMarkdown::Managed(chat.workspace_object());
        let chat_destination = resolve_for_write(&self.root, &chat.relative_path, operation)?;
        let chat_result = self
            .index
            .lock()
            .map_err(|_| lock_error(operation))
            .and_then(|mut index| {
                index.upsert_markdown(
                    &chat.relative_path,
                    &chat_bytes,
                    mtime_ns(&chat_destination),
                    &chat_parsed,
                )
            });
        let (chat_index_status, mut warnings) = self.index_outcome(chat_result);
        let message_parsed = ParsedMarkdown::Managed(message.workspace_object());
        let message_destination = resolve_for_write(&self.root, &message.relative_path, operation)?;
        let message_result = self
            .index
            .lock()
            .map_err(|_| lock_error(operation))
            .and_then(|mut index| {
                index.upsert_markdown(
                    &message.relative_path,
                    &message_bytes,
                    mtime_ns(&message_destination),
                    &message_parsed,
                )
            });
        let (message_index_status, message_warnings) = self.index_outcome(message_result);
        warnings.extend(message_warnings);
        Ok((chat_index_status, message_index_status, warnings))
    }

    pub(super) fn recover_pending_chat_mutations(&self) -> Result<()> {
        // Most calls find nothing to recover: check without the
        // cross-process lock first, then re-list under it.
        let directory = resolve_for_write(&self.root, CHAT_MUTATION_DIR, "chat_mutation_recover")?;
        let pending = std::fs::read_dir(&directory).is_ok_and(|mut entries| {
            entries.any(|entry| {
                entry.is_ok_and(|entry| {
                    entry.path().extension().and_then(|value| value.to_str()) == Some("json")
                })
            })
        });
        if !pending {
            return Ok(());
        }
        let _guard = self.write_lock("chat_mutation_recover")?;
        if !directory.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| CoreError::io(error, "chat_mutation_recover", directory.to_str()))?
        {
            let entry = entry.map_err(|error| {
                CoreError::io(error, "chat_mutation_recover", directory.to_str())
            })?;
            if !entry
                .file_type()
                .map_err(|error| CoreError::io(error, "chat_mutation_recover", directory.to_str()))?
                .is_file()
                || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let path = entry.path();
            let bytes = std::fs::read(&path)
                .map_err(|error| CoreError::io(error, "chat_mutation_recover", path.to_str()))?;
            let intent = serde_json::from_slice::<ChatMutationIntent>(&bytes).map_err(|_| {
                CoreError::new(
                    "chat_mutation_recovery_invalid",
                    ErrorCategory::Parse,
                    "A pending chat mutation intent is invalid",
                    "chat_mutation_recover",
                )
            })?;
            self.validate_chat_mutation_intent(&intent, "chat_mutation_recover")?;
            self.apply_chat_mutation_target(
                &intent.message,
                ChatMutationWriteTarget::Message,
                false,
                "chat_mutation_recover",
            )?;
            self.apply_chat_mutation_target(
                &intent.chat,
                ChatMutationWriteTarget::Chat,
                false,
                "chat_mutation_recover",
            )?;
            let _ = self.clear_chat_mutation_intent(&path, "chat_mutation_recover");
        }
        Ok(())
    }

    fn validate_chat_mutation_intent(
        &self,
        intent: &ChatMutationIntent,
        operation: &str,
    ) -> Result<()> {
        if intent.version != 1 {
            return Err(CoreError::new(
                "chat_mutation_recovery_invalid",
                ErrorCategory::Parse,
                "A pending chat mutation intent has an unsupported version",
                operation,
            ));
        }
        crate::path::validate_relative(&intent.chat.relative_path, operation)?;
        crate::path::validate_relative(&intent.message.relative_path, operation)?;
        let chat = parse_chat(&intent.chat.relative_path, intent.chat.bytes.as_bytes())?;
        let message = parse_chat_message(
            &intent.message.relative_path,
            intent.message.bytes.as_bytes(),
        )?;
        if message.chat_id != chat.id {
            return Err(CoreError::new(
                "chat_mutation_recovery_invalid",
                ErrorCategory::Parse,
                "A pending chat mutation does not join its chat and message",
                operation,
            ));
        }
        Ok(())
    }

    fn write_chat_mutation_intent(
        &self,
        intent: &ChatMutationIntent,
        operation: &str,
    ) -> Result<PathBuf> {
        let relative =
            PathBuf::from(CHAT_MUTATION_DIR).join(format!("{}.json", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec(intent).map_err(|_| {
            CoreError::new(
                "chat_mutation_serialize_failed",
                ErrorCategory::Parse,
                "Chat mutation intent could not be serialized",
                operation,
            )
        })?;
        atomic_write(&self.root, &relative, &bytes, operation)?;
        Ok(self.root.join(relative))
    }

    fn apply_chat_mutation_target(
        &self,
        target: &ChatMutationTarget,
        write_target: ChatMutationWriteTarget,
        inject_fault: bool,
        operation: &str,
    ) -> Result<()> {
        let relative = crate::path::validate_relative(&target.relative_path, operation)?;
        let destination = resolve_for_write(&self.root, &target.relative_path, operation)?;
        let target_revision = markdown::revision(target.bytes.as_bytes());
        match std::fs::read(&destination) {
            Ok(current) if markdown::revision(&current) == target_revision => return Ok(()),
            Ok(current)
                if target
                    .expected_revision
                    .as_deref()
                    .is_some_and(|expected| markdown::revision(&current) == expected) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && target.expected_revision.is_none() => {}
            Ok(_) | Err(_) => {
                return Err(CoreError::new(
                    "chat_mutation_recovery_conflict",
                    ErrorCategory::Conflict,
                    "A pending chat mutation would overwrite an external file change",
                    operation,
                ));
            }
        }
        if inject_fault && self.consume_chat_mutation_fault(write_target) {
            return Err(CoreError::new(
                "chat_mutation_write_failed",
                ErrorCategory::Filesystem,
                "A test fault interrupted the chat mutation before this file was written",
                operation,
            ));
        }
        atomic_write_checked(
            &self.root,
            &relative,
            target.bytes.as_bytes(),
            target.expected_revision.as_deref(),
            operation,
        )
    }

    fn journal_chat_mutation(&self, intent: &ChatMutationIntent) {
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(
                intent.chat.relative_path.clone(),
                markdown::revision(intent.chat.bytes.as_bytes()),
            );
            journal.insert(
                intent.message.relative_path.clone(),
                markdown::revision(intent.message.bytes.as_bytes()),
            );
        }
    }

    fn clear_chat_mutation_intent(&self, path: &Path, operation: &str) -> Result<()> {
        std::fs::remove_file(path)
            .map_err(|error| CoreError::io(error, operation, path.to_str()))?;
        sync_parent(path, operation)
    }

    fn consume_chat_mutation_fault(&self, target: ChatMutationWriteTarget) -> bool {
        let mut fault = self
            .chat_mutation_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(current) = fault.as_mut() else {
            return false;
        };
        if current.target != target {
            return false;
        }
        current.remaining -= 1;
        if current.remaining == 0 {
            *fault = None;
        }
        true
    }
}
