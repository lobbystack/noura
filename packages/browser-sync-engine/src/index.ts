/**
 * @noura/browser-sync-engine — local replica reconciliation for browser workspaces.
 *
 * The engine composes four injected boundaries — local {@link BrowserSyncStorage},
 * encrypted {@link BrowserSyncRemote}, an operation {@link FileChangeCodec}, and a
 * durable {@link SyncStateStore} — to:
 *
 * - seal local file changes into a durable outbox before any network call
 *   ({@link BrowserSyncEngine.enqueueFileChange});
 * - flush the outbox in order, then pull and apply version-1 file-change rules
 *   with expected revisions, recording conflicts without overwriting bytes
 *   ({@link BrowserSyncEngine.reconcile});
 * - resolve a recorded conflict by re-applying the stored encrypted remote
 *   operation or by enqueueing the local bytes as a replacement operation
 *   ({@link BrowserSyncEngine.resolveConflict});
 * - diff the local replica against the synchronized baseline so callers can
 *   enqueue added, changed, and deleted paths
 *   ({@link BrowserSyncEngine.snapshotLocalChanges});
 * - lock on a revoked codec or remote error via an injected `onRevoked` hook
 *   ({@link BrowserSyncEngine.lock}).
 *
 * It performs no cryptography, serialization, or network I/O itself, and it
 * never logs workspace plaintext or key material. A host must supply a durable
 * state store; {@link createFileSystemSyncStateStore} persists through an
 * injected filesystem at an adapter-owned path outside the canonical workspace,
 * while {@link createMemorySyncStateStore} and {@link MemorySyncStorage} are for
 * tests only.
 *
 * `BrowserWorkspaceStorage` can back {@link BrowserSyncStorage} through the
 * concrete {@link createWorkspaceStorageAdapter} (which derives `list` from
 * `rebuild()`) or the lower-level {@link createBrowserStorageAdapter}; OPFS
 * wiring, key rotation application, recovery kits, and UI remain outstanding.
 * See `docs/architecture/browser-sync.md`.
 */
export {
	BrowserSyncEngineError,
	BrowserSyncEngineErrorCode,
	REVOKED_ERROR_CODE,
	isRevokedError,
} from './errors';
export type { BrowserSyncEngineErrorOptions } from './errors';

export { BrowserSyncEngine, MAX_PUSH_BATCH } from './engine';
export type { BrowserSyncEngineOptions } from './engine';

export {
	cloneSyncState,
	createEmptySyncState,
	createFileSystemSyncStateStore,
	createMemorySyncStateStore,
	DEFAULT_SYNC_STATE_PATH,
	MAX_SYNC_STATE_BYTES,
	serializeSyncState,
	validateSyncState,
} from './state';
export { SYNC_STATE_VERSION } from './types';
export type { FileSystemSyncStateStore, SyncStateFileSystem } from './state';

export {
	MemorySyncStorage,
	MemorySyncStorageError,
	createBrowserStorageAdapter,
	createWorkspaceStorageAdapter,
	memoryRevision,
} from './storage';
export type {
	BrowserWorkspaceStorageLike,
	WorkspaceRebuildResult,
	WorkspaceStorageLike,
	WorkspaceStoredFile,
} from './storage';

export type {
	BrowserSyncEnginePhase,
	BrowserSyncRemote,
	BrowserSyncStorage,
	FileChange,
	FileChangeCodec,
	OpenedFileChange,
	ReconcileResult,
	ResolveConflictResult,
	SyncConflict,
	SyncConflictReason,
	SyncConflictResolution,
	SyncState,
	SyncStateMigration,
	SyncStateStore,
	SyncStorageDeleteInput,
	SyncStorageFile,
	SyncStorageMoveInput,
	SyncStorageWriteInput,
	ValidateSyncStateOptions,
} from './types';
