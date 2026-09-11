//! Native validation and materialization of Yjs update-v1 text documents.
use super::{crypto::decode, identifier, invalid};
use crate::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
#[cfg(not(test))]
use std::{
    process::{Command, Stdio},
    thread,
};
use yrs::{
    Any, Doc, GetString, OffsetKind, Options, Out, ReadTxn, StateVector, StickyIndex, Text,
    Transact, Update,
    updates::{decoder::Decode, encoder::Encode},
};

pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DOCUMENT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_WORKER_MEMORY_BYTES: usize = 512 * 1024 * 1024;
pub const WORKER_TIMEOUT: Duration = Duration::from_secs(5);
pub const TEXT_NAME: &str = "content";
const MAX_ACTIVE_WORKERS: usize = 16;
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationOpenInput {
    pub relative_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationSession {
    pub object_id: String,
    pub generation: String,
    pub session_id: String,
    pub update: String,
    pub revision: String,
    pub read_only: bool,
    pub role: CollaborationBootstrapRole,
    pub status: CollaborationStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum CollaborationBootstrapRole {
    Writer,
    Viewer,
}
#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationSubmitInput {
    pub session_id: String,
    pub generation: String,
    pub batch_id: String,
    pub updates: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationReceipt {
    pub revision: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub enum CollaborationStatus {
    Saving,
    #[serde(rename = "Saved locally")]
    #[ts(rename = "Saved locally")]
    SavedLocally,
    Synced,
    Offline,
    Reconnecting,
    #[serde(rename = "Needs review")]
    #[ts(rename = "Needs review")]
    NeedsReview,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationStatusEvent {
    pub object_id: String,
    pub generation: String,
    pub status: CollaborationStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationConflictReview {
    pub review_id: String,
    pub object_id: String,
    pub generation: String,
    pub reason: String,
    pub retained_draft: bool,
    pub competing_bytes: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborativeChange {
    pub version: u8,
    pub object_id: String,
    pub generation: String,
    pub updates: Vec<String>,
}
impl CollaborativeChange {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.object_id)?;
        identifier(&self.generation)?;
        if self.version != 1 {
            return Err(invalid("collaboration_unsupported_version"));
        }
        validate_updates(&self.updates)
    }
}

/// The decoder-owned representation. It never receives credentials, network handles, paths, or
/// workspace writers. Callers interact through [`TextDocument`]'s bounded worker boundary.
struct WorkerDocument {
    doc: Doc,
}
impl WorkerDocument {
    fn empty() -> Self {
        let doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        doc.get_or_insert_text(TEXT_NAME);
        Self { doc }
    }
    fn fresh_generation(text: &str, generation: &str) -> Result<Self> {
        identifier(generation)?;
        validate_text(text)?;
        let hash = blake3::hash(generation.as_bytes());
        let client_id = u32::from_le_bytes(
            hash.as_bytes()[..4]
                .try_into()
                .map_err(|_| invalid("collaboration_invalid_generation"))?,
        )
        .max(1);
        let doc = Doc::with_options(Options {
            client_id: yrs::ClientID::new(client_id as u64),
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        let field = doc.get_or_insert_text(TEXT_NAME);
        field.insert(&mut doc.transact_mut(), 0, text);
        Ok(Self { doc })
    }
    fn fresh(text: &str) -> Result<Self> {
        validate_text(text)?;
        let result = Self::empty();
        result
            .doc
            .get_or_insert_text(TEXT_NAME)
            .insert(&mut result.doc.transact_mut(), 0, text);
        Ok(result)
    }
    fn restore(state: &str) -> Result<Self> {
        let result = Self::empty();
        result.integrate(&decode(state, 2, MAX_DOCUMENT_BYTES)?)?;
        result.validate()?;
        Ok(result)
    }
    fn integrate(&self, bytes: &[u8]) -> Result<()> {
        let update =
            Update::decode_v1(bytes).map_err(|_| invalid("collaboration_invalid_update"))?;
        self.doc
            .transact_mut()
            .apply_update(update)
            .map_err(|_| invalid("collaboration_invalid_update"))
    }
    fn apply(&self, updates: &[String], deadline: Instant) -> Result<Self> {
        validate_updates(updates)?;
        let result = Self::restore(&self.state()?)?;
        for update in updates {
            check_deadline(deadline)?;
            result.integrate(&decode(update, 2, MAX_DOCUMENT_BYTES)?)?;
        }
        result.validate()?;
        result.state()?;
        Ok(result)
    }
    fn text(&self) -> String {
        self.doc
            .get_or_insert_text(TEXT_NAME)
            .get_string(&self.doc.transact())
    }
    fn state(&self) -> Result<String> {
        let bytes = self
            .doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(invalid("collaboration_history_limit"));
        }
        Ok(STANDARD.encode(bytes))
    }
    fn state_vector(&self) -> String {
        STANDARD.encode(self.doc.transact().state_vector().encode_v1())
    }
    fn diff(&self, state_vector: &str) -> Result<String> {
        let vector = StateVector::decode_v1(&decode(state_vector, 1, MAX_DOCUMENT_BYTES)?)
            .map_err(|_| invalid("collaboration_invalid_state_vector"))?;
        Ok(STANDARD.encode(self.doc.transact().encode_state_as_update_v1(&vector)))
    }
    /// Make the smallest single text splice, using UTF-16 offsets shared with the editor.
    fn replace_text(&self, next: &str) -> Result<(Self, String)> {
        validate_text(next)?;
        let result = Self::restore(&self.state()?)?;
        let prior = self.text();
        let common_prefix = prior
            .chars()
            .zip(next.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let old_tail = &prior[common_prefix..];
        let next_tail = &next[common_prefix..];
        let common_suffix = old_tail
            .chars()
            .rev()
            .zip(next_tail.chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let start = prior[..common_prefix].encode_utf16().count() as u32;
        let delete = old_tail[..old_tail.len() - common_suffix]
            .encode_utf16()
            .count() as u32;
        let insert = &next_tail[..next_tail.len() - common_suffix];
        {
            let text = result.doc.get_or_insert_text(TEXT_NAME);
            let mut txn = result.doc.transact_mut();
            if delete != 0 {
                text.remove_range(&mut txn, start, delete);
            }
            if !insert.is_empty() {
                text.insert(&mut txn, start, insert);
            }
        }
        let update = result.diff(&self.state_vector())?;
        result.validate()?;
        Ok((result, update))
    }
    fn validate(&self) -> Result<()> {
        let text = self.doc.get_or_insert_text(TEXT_NAME);
        let txn = self.doc.transact();
        if txn.has_missing_updates() {
            return Err(invalid("collaboration_missing_dependencies"));
        }
        for (name, value) in txn.root_refs() {
            if name != TEXT_NAME || !matches!(value, Out::YText(_)) {
                return Err(invalid("collaboration_unsupported_type"));
            }
        }
        for part in text.diff(&txn, |_| ()) {
            if part.attributes.is_some() || !matches!(part.insert, Out::Any(Any::String(_))) {
                return Err(invalid("collaboration_unsupported_type"));
            }
        }
        validate_text(&text.get_string(&txn))
    }

    fn validate_relative_positions(&self, positions: &[String]) -> Result<()> {
        let text = self.doc.get_or_insert_text(TEXT_NAME);
        let txn = self.doc.transact();
        for value in positions {
            let bytes = decode(value, 1, 512)?;
            let position = StickyIndex::decode_v1(&bytes)
                .map_err(|_| invalid("collaboration_invalid_presence"))?;
            let offset = position
                .get_offset(&txn)
                .ok_or_else(|| invalid("collaboration_invalid_presence"))?;
            if !std::ptr::eq(offset.branch.as_ref(), text.as_ref()) {
                return Err(invalid("collaboration_invalid_presence"));
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerSnapshot {
    state: String,
    state_vector: String,
    text: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum WorkerRequest {
    FreshGeneration {
        text: String,
        generation: String,
    },
    Fresh {
        text: String,
    },
    Restore {
        state: String,
    },
    Apply {
        state: String,
        updates: Vec<String>,
    },
    Diff {
        state: String,
        state_vector: String,
    },
    ReplaceText {
        state: String,
        text: String,
    },
    ValidateRelativePositions {
        state: String,
        positions: Vec<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum WorkerResponse {
    Snapshot {
        snapshot: WorkerSnapshot,
    },
    Diff {
        update: String,
    },
    ReplaceText {
        snapshot: WorkerSnapshot,
        update: String,
    },
    Valid,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerReply {
    response: Option<WorkerResponse>,
    error_code: Option<String>,
}

/// A validated CRDT snapshot. Every construction and candidate application runs on a dedicated,
/// time-bounded native worker that is given only immutable CRDT bytes and text.
pub struct TextDocument {
    state: String,
    state_vector: String,
    text: String,
}

impl TextDocument {
    pub fn fresh_generation(text: &str, generation: &str) -> Result<Self> {
        validate_text(text)?;
        identifier(generation)?;
        preflight_request_bytes(text.len())?;
        let WorkerResponse::Snapshot { snapshot } = run_worker(WorkerRequest::FreshGeneration {
            text: text.to_owned(),
            generation: generation.to_owned(),
        })?
        else {
            return Err(invalid("collaboration_worker_failed"));
        };
        Ok(Self::from_snapshot(snapshot))
    }

    pub fn fresh(text: &str) -> Result<Self> {
        validate_text(text)?;
        preflight_request_bytes(text.len())?;
        let WorkerResponse::Snapshot { snapshot } = run_worker(WorkerRequest::Fresh {
            text: text.to_owned(),
        })?
        else {
            return Err(invalid("collaboration_worker_failed"));
        };
        Ok(Self::from_snapshot(snapshot))
    }

    pub fn restore(state: &str) -> Result<Self> {
        preflight_encoded(state)?;
        preflight_request_bytes(state.len())?;
        let WorkerResponse::Snapshot { snapshot } = run_worker(WorkerRequest::Restore {
            state: state.to_owned(),
        })?
        else {
            return Err(invalid("collaboration_worker_failed"));
        };
        Ok(Self::from_snapshot(snapshot))
    }

    pub fn apply(&self, updates: &[String]) -> Result<Self> {
        preflight_encoded(&self.state)?;
        validate_updates(updates)?;
        preflight_request_bytes(
            self.state
                .len()
                .saturating_add(updates.iter().map(String::len).sum::<usize>()),
        )?;
        let WorkerResponse::Snapshot { snapshot } = run_worker(WorkerRequest::Apply {
            state: self.state.clone(),
            updates: updates.to_vec(),
        })?
        else {
            return Err(invalid("collaboration_worker_failed"));
        };
        Ok(Self::from_snapshot(snapshot))
    }

    pub fn text(&self) -> String {
        self.text.clone()
    }

    pub fn state(&self) -> Result<String> {
        preflight_encoded(&self.state)?;
        Ok(self.state.clone())
    }

    pub fn state_vector(&self) -> String {
        self.state_vector.clone()
    }

    pub fn diff(&self, state_vector: &str) -> Result<String> {
        preflight_encoded(&self.state)?;
        preflight_encoded(state_vector)?;
        preflight_request_bytes(self.state.len().saturating_add(state_vector.len()))?;
        let WorkerResponse::Diff { update } = run_worker(WorkerRequest::Diff {
            state: self.state.clone(),
            state_vector: state_vector.to_owned(),
        })?
        else {
            return Err(invalid("collaboration_worker_failed"));
        };
        Ok(update)
    }

    pub fn replace_text(&self, next: &str) -> Result<(Self, String)> {
        validate_text(next)?;
        preflight_encoded(&self.state)?;
        preflight_request_bytes(self.state.len().saturating_add(next.len()))?;
        let WorkerResponse::ReplaceText { snapshot, update } =
            run_worker(WorkerRequest::ReplaceText {
                state: self.state.clone(),
                text: next.to_owned(),
            })?
        else {
            return Err(invalid("collaboration_worker_failed"));
        };
        Ok((Self::from_snapshot(snapshot), update))
    }

    pub fn validate_relative_positions(&self, positions: &[String]) -> Result<()> {
        if positions.len() != 2 {
            return Err(invalid("collaboration_invalid_presence"));
        }
        for value in positions {
            preflight_encoded(value)?;
        }
        preflight_request_bytes(
            self.state
                .len()
                .saturating_add(positions.iter().map(String::len).sum::<usize>()),
        )?;
        match run_worker(WorkerRequest::ValidateRelativePositions {
            state: self.state.clone(),
            positions: positions.to_vec(),
        })? {
            WorkerResponse::Valid => Ok(()),
            _ => Err(invalid("collaboration_worker_failed")),
        }
    }

    fn from_snapshot(snapshot: WorkerSnapshot) -> Self {
        Self {
            state: snapshot.state,
            state_vector: snapshot.state_vector,
            text: snapshot.text,
        }
    }
}

fn snapshot(document: &WorkerDocument) -> Result<WorkerSnapshot> {
    Ok(WorkerSnapshot {
        state: document.state()?,
        state_vector: document.state_vector(),
        text: document.text(),
    })
}

struct ActiveWorkerGuard;
impl Drop for ActiveWorkerGuard {
    fn drop(&mut self) {
        ACTIVE_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

fn run_worker(request: WorkerRequest) -> Result<WorkerResponse> {
    ACTIVE_WORKERS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            (active < MAX_ACTIVE_WORKERS).then_some(active + 1)
        })
        .map_err(|_| invalid("collaboration_worker_busy"))?;
    let _guard = ActiveWorkerGuard;
    #[cfg(test)]
    {
        let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute_worker_request(request, Instant::now() + WORKER_TIMEOUT)
        }))
        .map_err(|_| invalid("collaboration_worker_failed"))??;
        round_trip_worker_result(response)
    }
    #[cfg(not(test))]
    run_worker_process(request)
}

#[cfg(test)]
fn round_trip_worker_result(value: WorkerResponse) -> Result<WorkerResponse> {
    let encoded = serde_json::to_vec(&value).map_err(|_| invalid("collaboration_worker_failed"))?;
    if encoded.len() > MAX_WORKER_MEMORY_BYTES / 4 {
        return Err(invalid("collaboration_worker_memory_limit"));
    }
    serde_json::from_slice(&encoded).map_err(|_| invalid("collaboration_worker_failed"))
}

fn execute_worker_request(request: WorkerRequest, deadline: Instant) -> Result<WorkerResponse> {
    check_deadline(deadline)?;
    match request {
        WorkerRequest::FreshGeneration { text, generation } => Ok(WorkerResponse::Snapshot {
            snapshot: snapshot(&WorkerDocument::fresh_generation(&text, &generation)?)?,
        }),
        WorkerRequest::Fresh { text } => Ok(WorkerResponse::Snapshot {
            snapshot: snapshot(&WorkerDocument::fresh(&text)?)?,
        }),
        WorkerRequest::Restore { state } => Ok(WorkerResponse::Snapshot {
            snapshot: snapshot(&WorkerDocument::restore(&state)?)?,
        }),
        WorkerRequest::Apply { state, updates } => {
            let document = WorkerDocument::restore(&state)?;
            Ok(WorkerResponse::Snapshot {
                snapshot: snapshot(&document.apply(&updates, deadline)?)?,
            })
        }
        WorkerRequest::Diff {
            state,
            state_vector,
        } => Ok(WorkerResponse::Diff {
            update: WorkerDocument::restore(&state)?.diff(&state_vector)?,
        }),
        WorkerRequest::ReplaceText { state, text } => {
            let (document, update) = WorkerDocument::restore(&state)?.replace_text(&text)?;
            Ok(WorkerResponse::ReplaceText {
                snapshot: snapshot(&document)?,
                update,
            })
        }
        WorkerRequest::ValidateRelativePositions { state, positions } => {
            WorkerDocument::restore(&state)?.validate_relative_positions(&positions)?;
            Ok(WorkerResponse::Valid)
        }
    }
}

#[cfg(not(test))]
fn run_worker_process(request: WorkerRequest) -> Result<WorkerResponse> {
    let encoded =
        serde_json::to_vec(&request).map_err(|_| invalid("collaboration_worker_failed"))?;
    if encoded.len() > MAX_WORKER_MEMORY_BYTES / 4 {
        return Err(invalid("collaboration_worker_memory_limit"));
    }
    let executable =
        std::env::current_exe().map_err(|_| invalid("collaboration_worker_unavailable"))?;
    let mut command = Command::new(executable);
    command
        .arg("--noura-crdt-worker")
        .env_clear()
        .current_dir(std::path::MAIN_SEPARATOR_STR)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(debug_assertions)]
    if std::env::var_os("NOURA_TEST_ALLOW_INHERITED_SANDBOX").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        command.env("NOURA_TEST_ALLOW_INHERITED_SANDBOX", "1");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: MAX_WORKER_MEMORY_BYTES as libc::rlim_t,
                rlim_max: MAX_WORKER_MEMORY_BYTES as libc::rlim_t,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|_| invalid("collaboration_worker_unavailable"))?;
    #[cfg(windows)]
    let _worker_job = windows_limit_worker(&child)?;
    child
        .stdin
        .take()
        .ok_or_else(|| invalid("collaboration_worker_unavailable"))?
        .write_all(&encoded)
        .map_err(|_| invalid("collaboration_worker_failed"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| invalid("collaboration_worker_unavailable"))?;
    let reader = thread::Builder::new()
        .name("noura-crdt-worker-output".into())
        .spawn(move || {
            let mut output = Vec::new();
            stdout
                .take((MAX_WORKER_MEMORY_BYTES / 4 + 1) as u64)
                .read_to_end(&mut output)
                .map(|_| output)
        })
        .map_err(|_| invalid("collaboration_worker_unavailable"))?;
    let deadline = Instant::now() + WORKER_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|_| invalid("collaboration_worker_failed"))?
        {
            break status;
        }
        #[cfg(target_os = "macos")]
        if macos_worker_memory(&child)? > MAX_WORKER_MEMORY_BYTES as u64 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(invalid("collaboration_worker_memory_limit"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(invalid("collaboration_worker_timeout"));
        }
        thread::sleep(Duration::from_millis(5));
    };
    let output = reader
        .join()
        .map_err(|_| invalid("collaboration_worker_failed"))?
        .map_err(|_| invalid("collaboration_worker_failed"))?;
    if !status.success() || output.len() > MAX_WORKER_MEMORY_BYTES / 4 {
        return Err(invalid("collaboration_worker_failed"));
    }
    let reply: WorkerReply =
        serde_json::from_slice(&output).map_err(|_| invalid("collaboration_worker_failed"))?;
    match (reply.response, reply.error_code) {
        (Some(response), None) => Ok(response),
        (None, Some(code)) => Err(crate::CoreError::validation(
            &code,
            "The collaboration worker rejected the request",
            "collaboration_worker",
        )),
        _ => Err(invalid("collaboration_worker_failed")),
    }
}

#[cfg(all(not(test), windows))]
struct WorkerJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(all(not(test), windows))]
impl Drop for WorkerJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(all(not(test), windows))]
fn windows_limit_worker(child: &std::process::Child) -> Result<WorkerJob> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        },
    };
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        return Err(invalid("collaboration_worker_unavailable"));
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    limits.ProcessMemoryLimit = MAX_WORKER_MEMORY_BYTES;
    let configured = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    } != 0;
    let assigned =
        configured && unsafe { AssignProcessToJobObject(job, child.as_raw_handle().cast()) } != 0;
    if !assigned {
        unsafe {
            CloseHandle(job);
        }
        return Err(invalid("collaboration_worker_unavailable"));
    }
    Ok(WorkerJob(job))
}

#[cfg(all(not(test), target_os = "macos"))]
fn macos_worker_memory(child: &std::process::Child) -> Result<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    let status = unsafe {
        libc::proc_pid_rusage(
            child.id() as libc::c_int,
            libc::RUSAGE_INFO_V2,
            usage.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return Err(invalid("collaboration_worker_unavailable"));
    }
    Ok(unsafe { usage.assume_init() }.ri_phys_footprint)
}

/// Run exactly one credential-free CRDT request from stdin and write one bounded reply to stdout.
/// Host executables call this before initializing networking, credentials, or workspace services.
pub fn run_worker_stdio() -> std::io::Result<()> {
    let mut encoded = Vec::new();
    std::io::stdin()
        .take((MAX_WORKER_MEMORY_BYTES / 4 + 1) as u64)
        .read_to_end(&mut encoded)?;
    restrict_worker_capabilities()?;
    let reply = if encoded.len() > MAX_WORKER_MEMORY_BYTES / 4 {
        WorkerReply {
            response: None,
            error_code: Some("collaboration_worker_memory_limit".into()),
        }
    } else {
        match serde_json::from_slice::<WorkerRequest>(&encoded)
            .map_err(|_| invalid("collaboration_worker_failed"))
            .and_then(|request| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_worker_request(request, Instant::now() + WORKER_TIMEOUT)
                }))
                .map_err(|_| invalid("collaboration_worker_failed"))?
            }) {
            Ok(response) => WorkerReply {
                response: Some(response),
                error_code: None,
            },
            Err(error) => WorkerReply {
                response: None,
                error_code: Some(error.code),
            },
        }
    };
    let response = serde_json::to_vec(&reply).map_err(std::io::Error::other)?;
    if response.len() > MAX_WORKER_MEMORY_BYTES / 4 {
        return Err(std::io::Error::other(
            "worker response exceeds memory limit",
        ));
    }
    std::io::stdout().write_all(&response)
}

#[cfg(target_os = "macos")]
fn restrict_worker_capabilities() -> std::io::Result<()> {
    use std::ffi::{CStr, c_char};

    unsafe extern "C" {
        static kSBXProfilePureComputation: c_char;
        fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> i32;
        fn sandbox_free_error(errorbuf: *mut c_char);
    }

    const SANDBOX_NAMED: u64 = 1;
    let mut error = std::ptr::null_mut();
    let status = unsafe {
        sandbox_init(
            &raw const kSBXProfilePureComputation,
            SANDBOX_NAMED,
            &mut error,
        )
    };
    if status == 0 {
        return Ok(());
    }
    let os_error = std::io::Error::last_os_error();
    let message = if error.is_null() {
        "could not enter the pure-computation sandbox".into()
    } else {
        let message = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        unsafe { sandbox_free_error(error) };
        message
    };
    if cfg!(debug_assertions)
        && os_error.raw_os_error() == Some(libc::EPERM)
        && std::env::var_os("NOURA_TEST_ALLOW_INHERITED_SANDBOX").as_deref()
            == Some(std::ffi::OsStr::new("1"))
    {
        return Ok(());
    }
    Err(std::io::Error::other(message))
}

#[cfg(target_os = "linux")]
fn restrict_worker_capabilities() -> std::io::Result<()> {
    #[cfg(target_arch = "x86_64")]
    const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
    #[cfg(target_arch = "aarch64")]
    const AUDIT_ARCH_AARCH64: u32 = 0xc000_00b7;
    const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
    const BPF_LD_W_ABS: u16 = 0x20;
    const BPF_JMP_JEQ_K: u16 = 0x15;
    const BPF_RET_K: u16 = 0x06;

    #[cfg(target_arch = "x86_64")]
    const CURRENT_ARCH: u32 = AUDIT_ARCH_X86_64;
    #[cfg(target_arch = "aarch64")]
    const CURRENT_ARCH: u32 = AUDIT_ARCH_AARCH64;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    compile_error!("the collaboration worker sandbox needs an audit architecture constant");

    fn statement(code: u16, value: u32) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k: value,
        }
    }
    fn jump(code: u16, value: u32, yes: u8, no: u8) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: yes,
            jf: no,
            k: value,
        }
    }

    let denied = [
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_sendto,
        libc::SYS_recvfrom,
        libc::SYS_sendmsg,
        libc::SYS_recvmsg,
        libc::SYS_shutdown,
        libc::SYS_setsockopt,
        libc::SYS_getsockopt,
        libc::SYS_open,
        libc::SYS_openat,
        libc::SYS_openat2,
        libc::SYS_creat,
        libc::SYS_unlink,
        libc::SYS_unlinkat,
        libc::SYS_rename,
        libc::SYS_renameat,
        libc::SYS_renameat2,
        libc::SYS_mkdir,
        libc::SYS_mkdirat,
        libc::SYS_rmdir,
        libc::SYS_link,
        libc::SYS_linkat,
        libc::SYS_symlink,
        libc::SYS_symlinkat,
        libc::SYS_mknod,
        libc::SYS_mknodat,
        libc::SYS_chmod,
        libc::SYS_fchmod,
        libc::SYS_fchmodat,
        libc::SYS_chown,
        libc::SYS_fchown,
        libc::SYS_lchown,
        libc::SYS_fchownat,
        libc::SYS_truncate,
        libc::SYS_ftruncate,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_name_to_handle_at,
        libc::SYS_open_by_handle_at,
        libc::SYS_execve,
        libc::SYS_execveat,
        libc::SYS_fork,
        libc::SYS_vfork,
        libc::SYS_clone,
        libc::SYS_clone3,
    ];
    let mut filter = Vec::with_capacity(denied.len() * 2 + 5);
    filter.push(statement(BPF_LD_W_ABS, 4));
    filter.push(jump(BPF_JMP_JEQ_K, CURRENT_ARCH, 1, 0));
    filter.push(statement(BPF_RET_K, SECCOMP_RET_KILL_PROCESS));
    filter.push(statement(BPF_LD_W_ABS, 0));
    for syscall in denied {
        filter.push(jump(BPF_JMP_JEQ_K, syscall as u32, 0, 1));
        filter.push(statement(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32));
    }
    filter.push(statement(BPF_RET_K, SECCOMP_RET_ALLOW));
    let program = libc::sock_fprog {
        len: filter
            .len()
            .try_into()
            .map_err(|_| std::io::Error::other("worker sandbox program is too large"))?,
        filter: filter.as_mut_ptr(),
    };
    let no_new_privileges = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if no_new_privileges != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let installed = unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            &raw const program,
        )
    };
    if installed != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn restrict_worker_capabilities() -> std::io::Result<()> {
    Err(std::io::Error::other(
        "the Windows collaboration worker requires AppContainer confinement",
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn restrict_worker_capabilities() -> std::io::Result<()> {
    Err(std::io::Error::other(
        "the collaboration worker is not confined on this platform",
    ))
}

fn check_deadline(deadline: Instant) -> Result<()> {
    if Instant::now() > deadline {
        return Err(invalid("collaboration_worker_timeout"));
    }
    Ok(())
}

fn preflight_encoded(value: &str) -> Result<()> {
    if value.len() > MAX_DOCUMENT_BYTES * 4 / 3 + 4 {
        return Err(invalid("collaboration_history_limit"));
    }
    Ok(())
}

fn preflight_request_bytes(encoded_bytes: usize) -> Result<()> {
    // Yrs update-v1 is not compressed. Reserve an 8x expansion budget for decoded structs,
    // indexes, candidate state, and response serialization before starting the worker.
    if encoded_bytes > MAX_WORKER_MEMORY_BYTES / 8 {
        return Err(invalid("collaboration_worker_memory_limit"));
    }
    Ok(())
}
fn validate_updates(updates: &[String]) -> Result<()> {
    if updates.is_empty()
        || updates.len() > 100
        || updates.iter().map(String::len).sum::<usize>() > MAX_DOCUMENT_BYTES * 4 / 3 + 4
    {
        return Err(invalid("collaboration_update_limit"));
    }
    for update in updates {
        decode(update, 2, MAX_DOCUMENT_BYTES)?;
    }
    Ok(())
}
pub(crate) fn validate_update_batch(updates: &[String]) -> Result<()> {
    validate_updates(updates)
}
pub fn validate_text(text: &str) -> Result<()> {
    if text.len() > MAX_TEXT_BYTES || text.contains('\0') {
        return Err(invalid("collaboration_unsupported_text"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yjs_fixture_edits_deletions_and_local_undo_interoperate() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/yjs-text-v1.json"
        ))
        .unwrap();
        let mut doc = TextDocument::restore(value["base"].as_str().unwrap()).unwrap();
        for (update, expected) in value["updates"]
            .as_array()
            .unwrap()
            .iter()
            .zip(value["states"].as_array().unwrap())
        {
            doc = doc.apply(&[update.as_str().unwrap().into()]).unwrap();
            assert_eq!(doc.text(), expected.as_str().unwrap());
        }
        assert_eq!(doc.text(), value["text"].as_str().unwrap());
        let vector = value["vector"].as_str().unwrap();
        let delta = doc.diff(vector).unwrap();
        let restored = TextDocument::restore(value["state"].as_str().unwrap()).unwrap();
        assert_eq!(restored.apply(&[delta]).unwrap().text(), doc.text());
    }
    #[test]
    fn eight_mib_text_boundary_survives_binary_state_restore() {
        let text = "x".repeat(MAX_TEXT_BYTES);
        let doc = TextDocument::fresh_generation(&text, "boundary").unwrap();
        let restored = TextDocument::restore(&doc.state().unwrap()).unwrap();
        assert_eq!(restored.text().len(), MAX_TEXT_BYTES);
        let oversized = format!("{text}x");
        assert!(restored.replace_text(&oversized).is_err());
        assert_eq!(restored.text().len(), MAX_TEXT_BYTES);
    }
    #[test]
    fn unicode_replacement_preserves_utf16_offsets() {
        let doc = TextDocument::fresh("A😀B\n").unwrap();
        let (updated, delta) = doc.replace_text("A😀中文B\n").unwrap();
        assert_eq!(doc.apply(&[delta]).unwrap().text(), updated.text());
        assert_eq!(doc.text(), "A😀B\n");
    }
    #[test]
    fn replicas_merge_concurrent_edits_without_duplicate_bootstrap() {
        let base = TextDocument::fresh("start end").unwrap();
        let (left, left_update) = base.replace_text("LEFT start end").unwrap();
        let (right, right_update) = base.replace_text("start end RIGHT").unwrap();
        let left = left.apply(&[right_update]).unwrap();
        let right = right.apply(&[left_update]).unwrap();
        assert_eq!(left.text(), "LEFT start end RIGHT");
        assert_eq!(left.text(), right.text());
        assert_eq!(
            left.apply(&[left.state().unwrap()]).unwrap().text(),
            left.text()
        );
    }
    #[test]
    fn invalid_updates_leave_accepted_state_unchanged() {
        let doc = TextDocument::fresh("accepted").unwrap();
        assert!(doc.apply(&[STANDARD.encode([255, 255, 255])]).is_err());
        let other = Doc::new();
        other.get_or_insert_map("secrets");
        use yrs::Map;
        other
            .get_or_insert_map("secrets")
            .insert(&mut other.transact_mut(), "key", "value");
        let bytes = other
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        assert!(doc.apply(&[STANDARD.encode(bytes)]).is_err());

        let formatted = Doc::new();
        let formatted_text = formatted.get_or_insert_text(TEXT_NAME);
        use yrs::types::Attrs;
        formatted_text.insert_with_attributes(
            &mut formatted.transact_mut(),
            0,
            "formatted",
            Attrs::from([("bold".into(), true.into())]),
        );
        let bytes = formatted
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        assert!(doc.apply(&[STANDARD.encode(bytes)]).is_err());

        let embedded = Doc::new();
        embedded.get_or_insert_text(TEXT_NAME).insert_embed(
            &mut embedded.transact_mut(),
            0,
            Any::Number(1.0),
        );
        let bytes = embedded
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        assert!(doc.apply(&[STANDARD.encode(bytes)]).is_err());

        let dependent = Doc::new();
        let dependent_text = dependent.get_or_insert_text(TEXT_NAME);
        dependent_text.insert(&mut dependent.transact_mut(), 0, "base");
        let vector = dependent.transact().state_vector();
        let first = dependent
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        dependent_text.insert(&mut dependent.transact_mut(), 4, " later");
        let second = dependent.transact().encode_state_as_update_v1(&vector);
        assert!(doc.apply(&[STANDARD.encode(&second)]).is_err());
        assert_eq!(doc.text(), "accepted");

        let reordered = TextDocument::fresh("")
            .unwrap()
            .apply(&[
                STANDARD.encode(&second),
                STANDARD.encode(&first),
                STANDARD.encode(&second),
            ])
            .unwrap();
        assert_eq!(reordered.text(), "base later");
    }
}
