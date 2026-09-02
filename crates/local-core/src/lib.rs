#![allow(
    clippy::result_large_err,
    reason = "CoreError is a deliberately structured, serializable IPC error; errors are exceptional paths"
)]

mod ai;
mod engine;
mod error;
mod index;
mod markdown;
mod model;
mod path;
mod watcher;

pub use ai::*;
pub use engine::{
    ConflictResolution, CreateObjectInput, DraftReconcileInput, DraftReconcileResult,
    ManagedConflictResolution, ManagedConflictResolveInput, ManagedDraftInput, ManagedDraftResult,
    ManifestUpdateInput, MarkdownLinkTarget, ObjectPatch, RawConflictResolveInput,
    RawConflictResolveResult, RawMarkdownRead, RawReconcileInput, RawReconcileResult, RawSaveInput,
    RawSaveResult, ResolveConflictInput, WorkspaceEngine,
};
pub use error::{CoreError, ErrorCategory, Result};
pub use index::{CalendarEntry, IndexStore, SearchInput, SearchResult};
pub use markdown::{ParsedMarkdown, parse_markdown, serialize_object};
pub use model::*;
pub use watcher::WatchCoordinator;
