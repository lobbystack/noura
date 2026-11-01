//! The chat mutation journal. A chat step writes two files, so the engine
//! records its intent first and replays it after a crash.

use super::*;

const CHAT_MUTATION_DIR: &str = ".noura/chat-mutations";
/// The extension of an intent replay set aside: replay no longer lists it,
/// and its file keeps the bytes the change meant to write.
const SET_ASIDE_EXTENSION: &str = "unapplied";

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
    Intent,
    Chat,
    Message,
}

/// What a target file holds, as an intent sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatTargetState {
    /// The intent's new bytes.
    Written,
    /// What the intent expected to replace, or nothing where it adds a file.
    Waiting,
    /// Bytes the intent never expected.
    Changed,
    /// Nothing, where the intent expected a file.
    Missing,
}

/// How a replay left its intent's change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatReplay {
    /// Every file that could take the change holds it.
    Finished,
    /// A file the change can't go on top of keeps its bytes, and the intent
    /// goes aside with the bytes it couldn't write.
    SetAside,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ChatMutationFault {
    target: ChatMutationWriteTarget,
    remaining: usize,
}

/// An intent the last replay couldn't finish. The file stays on disk and
/// stays the authority: every replay rebuilds this list from the files.
#[derive(Debug, Clone)]
pub(super) struct StuckChatMutation {
    intent: PathBuf,
    /// The chat the intent's bytes name, when they parse.
    chat_id: Option<String>,
    /// The chat file the intent names, when its JSON parses.
    chat_path: Option<String>,
    error: CoreError,
}

impl StuckChatMutation {
    fn names(&self, chat: &Chat) -> bool {
        self.chat_id.as_deref() == Some(chat.id.as_str())
            || self.chat_path.as_deref() == Some(chat.relative_path.as_str())
    }

    /// Whether the chat's files can't be shown as they stand. Replay can't
    /// finish a broken intent by itself. A file that waits for a download
    /// or a disk that refuses a write can pass, and the files read as they
    /// are until then.
    fn holds_reads(&self) -> bool {
        matches!(
            self.error.category,
            ErrorCategory::Parse | ErrorCategory::Validation
        )
    }
}

/// A place in chat recovery where a test can act.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRecoveryPoint {
    /// After recovery's walk, before it takes the write lock.
    BeforeWriteLock,
    /// Under the write lock, after recovery's deciding read.
    AfterDecidingRead,
    /// Just after an intent's file goes, before its folder syncs.
    IntentRemoved,
    /// During a commit, after the intent reached disk, before either file
    /// changes.
    IntentWritten,
    /// During a commit, after the message file, before the chat file.
    MessageWritten,
}

#[cfg(any(test, feature = "test-hooks"))]
type ChatRecoveryHook = Box<dyn FnOnce(&WorkspaceEngine) + Send>;

/// What tests watch and steer in chat recovery.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Default)]
pub(super) struct ChatRecoveryProbe {
    hooks: Mutex<Vec<(ChatRecoveryPoint, ChatRecoveryHook)>>,
    pub(super) walks: AtomicU64,
}

impl WorkspaceEngine {
    /// Test hook: run `hook` once, the next time recovery reaches `point`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn on_chat_recovery_point(
        &self,
        point: ChatRecoveryPoint,
        hook: impl FnOnce(&WorkspaceEngine) + Send + 'static,
    ) {
        self.chat_recovery_probe
            .hooks
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((point, Box::new(hook)));
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn chat_recovery_checkpoint(&self, point: ChatRecoveryPoint) {
        let hook = {
            let mut hooks = self
                .chat_recovery_probe
                .hooks
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            hooks
                .iter()
                .position(|(at, _)| *at == point)
                .map(|position| hooks.remove(position).1)
        };
        if let Some(hook) = hook {
            hook(self);
        }
    }

    /// Test hook: how many workspace walks this engine has started.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn workspace_walks_for_testing(&self) -> u64 {
        self.chat_recovery_probe.walks.load(Ordering::SeqCst)
    }

    /// Test hook: whether the next look at the index walks the workspace.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn index_marked_stale_for_testing(&self) -> bool {
        self.dirty.load(Ordering::SeqCst)
    }

    /// Test hook for exercising recovery after either replacement in the
    /// multi-file chat commit. The fault is consumed before the selected write.
    #[doc(hidden)]
    pub fn fail_chat_mutation_write_for_testing(&self, target: &str, times: usize) {
        let target = match target {
            "intent" => ChatMutationWriteTarget::Intent,
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
    ///
    /// The chat file's half of every commit is its `updated` time, which
    /// [`Self::replay_chat_mutation`] relies on. A file that changes while
    /// this runs fails it with `revision_conflict`, and this writer takes
    /// back what it wrote, so no intent outlives a conflict it can report.
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
        let prior_message = expected_message_revision
            .map(|_| {
                let path = resolve_for_write(&self.root, &message.relative_path, operation)?;
                read_downloaded(&path, &message.relative_path, operation)
            })
            .transpose()?;
        let intent_path = self.write_chat_mutation_intent(&intent, operation)?;
        #[cfg(any(test, feature = "test-hooks"))]
        self.chat_recovery_checkpoint(ChatRecoveryPoint::IntentWritten);

        if let Err(error) = self.apply_chat_mutation(&intent, operation) {
            if error.code == "revision_conflict" {
                self.undo_chat_mutation(
                    &intent,
                    &intent_path,
                    prior_message.as_deref(),
                    operation,
                )?;
            }
            return Err(error);
        }
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
        let (chat_index_status, mut warnings) = self.chat_index_outcome(chat_result);
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
        let (message_index_status, message_warnings) = self.chat_index_outcome(message_result);
        warnings.extend(message_warnings);
        Ok((chat_index_status, message_index_status, warnings))
    }

    /// Check both files once more, now that the intent's sync has given
    /// them time to change, then write the child first. Neither replacement
    /// is a successful mutation until recovery has observed both recorded
    /// target revisions.
    fn apply_chat_mutation(&self, intent: &ChatMutationIntent, operation: &str) -> Result<()> {
        for target in [&intent.message, &intent.chat] {
            if matches!(
                self.chat_target_state(target, operation)?,
                ChatTargetState::Changed | ChatTargetState::Missing
            ) {
                return Err(stale_chat_target(operation));
            }
        }
        self.journal_chat_mutation(intent);
        self.apply_chat_mutation_target(
            &intent.message,
            ChatMutationWriteTarget::Message,
            true,
            operation,
        )?;
        #[cfg(any(test, feature = "test-hooks"))]
        self.chat_recovery_checkpoint(ChatRecoveryPoint::MessageWritten);
        self.apply_chat_mutation_target(
            &intent.chat,
            ChatMutationWriteTarget::Chat,
            true,
            operation,
        )
    }

    /// Take back a commit that lost to a file changing under it: put the
    /// message file back as it was and delete the intent. A message file
    /// that no longer holds this commit's bytes stays as it is. When this
    /// fails the intent stays, and replay settles the change.
    fn undo_chat_mutation(
        &self,
        intent: &ChatMutationIntent,
        intent_path: &Path,
        prior_message: Option<&[u8]>,
        operation: &str,
    ) -> Result<()> {
        let target = &intent.message;
        if self.chat_target_state(target, operation)? == ChatTargetState::Written {
            let relative = crate::path::validate_relative(&target.relative_path, operation)?;
            match prior_message {
                Some(bytes) => {
                    let written = markdown::revision(target.bytes.as_bytes());
                    match atomic_write_checked(
                        &self.root,
                        &relative,
                        bytes,
                        Some(&written),
                        operation,
                    ) {
                        Err(error) if error.code != "revision_conflict" => return Err(error),
                        Ok(()) | Err(_) => {}
                    }
                }
                None => {
                    let path = resolve_for_write(&self.root, &target.relative_path, operation)?;
                    std::fs::remove_file(&path)
                        .map_err(|error| CoreError::io(error, operation, path.to_str()))?;
                    sync_parent(&path, operation)?;
                }
            }
        }
        self.forget_own_write(&intent.chat.relative_path);
        self.forget_own_write(&intent.message.relative_path);
        // The index never saw this commit, and it may not have seen the
        // change that beat it. Marked before the intent goes, as replay does.
        self.dirty.store(true, Ordering::SeqCst);
        self.clear_chat_mutation_intent(intent_path, operation)
    }

    /// Replay what a crash or a failed write left. An intent replay can't
    /// finish stops its own chat and no other: it stays on disk, the next
    /// call tries it again, and [`Self::stuck_chat_mutation_error`] answers
    /// for that chat. A file that changed outside Noura stops nothing, as
    /// [`Self::replay_chat_mutation`] says. This fails only when the intents
    /// can't be listed.
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
            self.stuck_chat_mutations_guard().clear();
            return Ok(());
        }
        let _guard = self.write_lock("chat_mutation_recover")?;
        self.replay_chat_mutations_unlocked()
    }

    /// [`Self::recover_pending_chat_mutations`] for a caller that holds the
    /// write lock.
    pub(super) fn replay_chat_mutations_unlocked(&self) -> Result<()> {
        let directory = resolve_for_write(&self.root, CHAT_MUTATION_DIR, "chat_mutation_recover")?;
        let mut stuck = Vec::new();
        if directory.exists() {
            for entry in std::fs::read_dir(&directory).map_err(|error| {
                CoreError::io(error, "chat_mutation_recover", directory.to_str())
            })? {
                let entry = entry.map_err(|error| {
                    CoreError::io(error, "chat_mutation_recover", directory.to_str())
                })?;
                if !entry
                    .file_type()
                    .map_err(|error| {
                        CoreError::io(error, "chat_mutation_recover", directory.to_str())
                    })?
                    .is_file()
                    || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
                {
                    continue;
                }
                let path = entry.path();
                let relative = format!(
                    "{CHAT_MUTATION_DIR}/{}",
                    entry.file_name().to_string_lossy()
                );
                match self.recover_chat_mutation(&path, &relative) {
                    Ok(replay) => {
                        // The index never recorded the files the change
                        // wrote, so the next look at the index walks the
                        // workspace. Marked before the intent goes, so a
                        // reader that finds no intent finds the mark.
                        self.dirty.store(true, Ordering::SeqCst);
                        let _ = match replay {
                            ChatReplay::Finished => {
                                self.clear_chat_mutation_intent(&path, "chat_mutation_recover")
                            }
                            ChatReplay::SetAside => self.set_aside_chat_mutation_intent(&path),
                        };
                    }
                    Err(failure) => stuck.push(failure),
                }
            }
        }
        let mut known = self.stuck_chat_mutations_guard();
        for failure in &stuck {
            if !known.iter().any(|old| old.intent == failure.intent) {
                tracing::warn!(
                    code = %failure.error.code,
                    intent = ?failure.intent,
                    "a pending chat mutation stays on disk"
                );
            }
        }
        *known = stuck;
        Ok(())
    }

    /// The error for using `chat` while an intent for it stays unfinished:
    /// for a read, when its files can't be shown as they stand, and for a
    /// write, whatever stopped the replay, because a write on top would
    /// turn a replay that can still finish into a conflict.
    pub(super) fn stuck_chat_mutation_error(&self, chat: &Chat, write: bool) -> Option<CoreError> {
        self.stuck_chat_mutations_guard()
            .iter()
            .find(|stuck| stuck.names(chat) && (write || stuck.holds_reads()))
            .map(|stuck| stuck.error.clone())
    }

    fn stuck_chat_mutations_guard(&self) -> std::sync::MutexGuard<'_, Vec<StuckChatMutation>> {
        self.stuck_chat_mutations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Replay the intent at `path`, which `relative` names, onto the chat
    /// and message files it names.
    fn recover_chat_mutation(
        &self,
        path: &Path,
        relative: &str,
    ) -> std::result::Result<ChatReplay, StuckChatMutation> {
        let stuck = |chat_id: Option<String>, chat_path: Option<String>, error: CoreError| {
            StuckChatMutation {
                intent: path.to_owned(),
                chat_id,
                chat_path,
                error,
            }
        };
        let bytes = read_downloaded(path, relative, "chat_mutation_recover")
            .map_err(|error| stuck(None, None, error))?;
        let intent = serde_json::from_slice::<ChatMutationIntent>(&bytes).map_err(|_| {
            stuck(
                None,
                None,
                CoreError::new(
                    "chat_mutation_recovery_invalid",
                    ErrorCategory::Parse,
                    "A pending chat mutation intent is invalid",
                    "chat_mutation_recover",
                ),
            )
        })?;
        let chat_path = Some(intent.chat.relative_path.clone());
        self.validate_chat_mutation_intent(&intent, "chat_mutation_recover")
            .map_err(|error| stuck(None, chat_path.clone(), error))?;
        let chat_id = parse_chat(&intent.chat.relative_path, intent.chat.bytes.as_bytes())
            .ok()
            .map(|chat| chat.id);
        self.replay_chat_mutation(&intent)
            .map_err(|error| stuck(chat_id, chat_path, error))
    }

    /// Finish `intent`'s change around whatever changed outside Noura, so a
    /// crash and an outside edit never leave a chat you can't open. Files
    /// win. The chat file's half holds only an `updated` time, so a chat
    /// file that changed keeps its bytes and the message still goes in. A
    /// message file that changed, or a chat that left its path, keeps what
    /// it holds, and the intent goes aside with the bytes it couldn't write.
    fn replay_chat_mutation(&self, intent: &ChatMutationIntent) -> Result<ChatReplay> {
        let operation = "chat_mutation_recover";
        if self.chat_target_state(&intent.chat, operation)? == ChatTargetState::Missing {
            return Ok(ChatReplay::SetAside);
        }
        match self.apply_chat_mutation_target(
            &intent.message,
            ChatMutationWriteTarget::Message,
            false,
            operation,
        ) {
            Err(error) if error.code == "revision_conflict" => return Ok(ChatReplay::SetAside),
            result => result?,
        }
        match self.apply_chat_mutation_target(
            &intent.chat,
            ChatMutationWriteTarget::Chat,
            false,
            operation,
        ) {
            Err(error) if error.code != "revision_conflict" => Err(error),
            Ok(()) | Err(_) => Ok(ChatReplay::Finished),
        }
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
        if self.consume_chat_mutation_fault(ChatMutationWriteTarget::Intent) {
            return Err(CoreError::new(
                "chat_mutation_write_failed",
                ErrorCategory::Filesystem,
                "A test fault stopped the chat mutation intent from being written",
                operation,
            ));
        }
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
        match self.chat_target_state(target, operation)? {
            ChatTargetState::Written => return Ok(()),
            ChatTargetState::Waiting => {}
            ChatTargetState::Changed | ChatTargetState::Missing => {
                return Err(stale_chat_target(operation));
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

    /// What `target`'s file holds. A file that can't be read fails, so a
    /// refused read never passes for a change.
    fn chat_target_state(
        &self,
        target: &ChatMutationTarget,
        operation: &str,
    ) -> Result<ChatTargetState> {
        let destination = resolve_for_write(&self.root, &target.relative_path, operation)?;
        // A file that lives only in the cloud stays unread, and the intent
        // waits until it downloads.
        ensure_downloaded(&destination, &target.relative_path, operation)?;
        match std::fs::read(&destination) {
            Ok(current) => {
                let current = markdown::revision(&current);
                if current == markdown::revision(target.bytes.as_bytes()) {
                    Ok(ChatTargetState::Written)
                } else if target.expected_revision.as_deref() == Some(current.as_str()) {
                    Ok(ChatTargetState::Waiting)
                } else {
                    Ok(ChatTargetState::Changed)
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(if target.expected_revision.is_none() {
                    ChatTargetState::Waiting
                } else {
                    ChatTargetState::Missing
                })
            }
            Err(error) => Err(CoreError::io(error, operation, destination.to_str())),
        }
    }

    fn journal_chat_mutation(&self, intent: &ChatMutationIntent) {
        self.expect_own_write(
            &intent.chat.relative_path,
            Some(&markdown::revision(intent.chat.bytes.as_bytes())),
        );
        self.expect_own_write(
            &intent.message.relative_path,
            Some(&markdown::revision(intent.message.bytes.as_bytes())),
        );
    }

    fn clear_chat_mutation_intent(&self, path: &Path, operation: &str) -> Result<()> {
        std::fs::remove_file(path)
            .map_err(|error| CoreError::io(error, operation, path.to_str()))?;
        #[cfg(any(test, feature = "test-hooks"))]
        self.chat_recovery_checkpoint(ChatRecoveryPoint::IntentRemoved);
        sync_parent(path, operation)
    }

    /// Move the intent at `path` out of replay's way, keeping its bytes.
    fn set_aside_chat_mutation_intent(&self, path: &Path) -> Result<()> {
        let operation = "chat_mutation_recover";
        tracing::warn!(intent = ?path, "a chat mutation met a changed file and went aside");
        let aside = path.with_extension(SET_ASIDE_EXTENSION);
        std::fs::rename(path, &aside)
            .map_err(|error| CoreError::io(error, operation, path.to_str()))?;
        sync_parent(&aside, operation)
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

/// The error for a target file that changed while its commit ran.
fn stale_chat_target(operation: &str) -> CoreError {
    CoreError::new(
        "revision_conflict",
        ErrorCategory::Conflict,
        "The file changed since it was loaded",
        operation,
    )
}
