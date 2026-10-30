#![allow(
    clippy::result_large_err,
    reason = "CoreError is a deliberately structured, serializable IPC error; errors are exceptional paths"
)]

mod ai;
mod chat;
mod consent;
mod credentials;
#[cfg(any(test, feature = "durability-probe"))]
pub mod durability_probe;
mod durable_settings;
mod engine;
mod error;
mod index;
mod markdown;
mod model;
mod path;
mod pdf;
mod text_files;
pub use pdf::{PdfInfo, PdfRangeInput};
mod remote_images;
pub use remote_images::{
    MAX_REMOTE_IMAGE_BYTES, MAX_REMOTE_IMAGE_REDIRECTS, REMOTE_IMAGE_TIMEOUT, RemoteImage,
    RemoteImageFetcher,
};
pub use text_files::is_plain_text_path;
pub mod sync;
mod watcher;
mod web_access;
mod web_search;

pub use ai::*;
pub use chat::*;
pub use consent::{
    AiConsentDataCategory, AiConsentGrant, AiConsentGrantInput, AiConsentReadInput,
    AiConsentRevokeInput, AiConsentRevokeOutcome,
};
pub use credentials::{CredentialStore, MemoryCredentials, NoCredentials, SystemCredentials};
pub use engine::{
    ChangeCounts, ConflictResolution, CreateObjectInput, DraftReconcileInput, DraftReconcileResult,
    ImportResult, ManagedConflictResolution, ManagedConflictResolveInput, ManagedDraftInput,
    ManagedDraftResult, ManifestUpdateInput, MarkdownLinkTarget, ObjectPatch,
    RawConflictResolveInput, RawConflictResolveResult, RawMarkdownRead, RawReconcileInput,
    RawReconcileResult, RawRestoreInput, RawSaveInput, RawSaveResult, ResolveConflictInput,
    SYNC_PLUGIN_ID, SkipReason, SkippedEntry, SyncedExpect, SyncedFile, SyncedListing, SystemTrash,
    WatchPoll, WorkspaceEngine, is_online_only, os_trash,
};
pub use error::{CoreError, ErrorCategory, Result};
pub use index::{
    IdentityConflict, IndexStore, ObjectFilter, ObjectSummary, ObjectSummaryQuery, PropertyValue,
    SearchInput, SearchResult,
};
pub use markdown::{ParsedMarkdown, parse_markdown, serialize_object, serialize_object_over};
pub use model::*;
pub use watcher::WatchCoordinator;
#[cfg(any(test, feature = "test-hooks"))]
pub use watcher::WatchFault;
pub use web_access::*;
pub use web_search::{BraveClient, WEB_SEARCH_PROVIDER, WebClient, WebConsentGrant, WebFoundation};
pub use workspace_format::merge::merge_text;
pub use workspace_format::{
    MAX_FILE_NAME_CHARS, WORKSPACE_MANIFEST_PATH, WorkspaceManifest, WorkspaceObject,
    frontmatter_end, normalize_file_name, title_from_path, valid_object_id, valid_object_type,
};
