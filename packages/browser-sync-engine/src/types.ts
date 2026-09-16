/**
 * Injectable boundaries and durable state types for {@link BrowserSyncEngine}.
 *
 * The engine owns reconciliation orchestration only. It performs no
 * cryptography, no workspace-format serialization, and no network I/O; each of
 * those lives behind the {@link FileChangeCodec}, {@link BrowserSyncStorage},
 * and {@link BrowserSyncRemote} boundaries supplied by the host.
 */
import type { EncryptedOperation, SyncPage } from '@noura/shared';
import type { FileChangeBlob } from '@noura/browser-sync';

/** A stored canonical file and its opaque content revision. */
export interface SyncStorageFile {
	bytes: Uint8Array;
	revision: string;
}

/** Input for {@link BrowserSyncStorage.write}. */
export interface SyncStorageWriteInput {
	path: string;
	bytes: Uint8Array;
	/**
	 * `null` requires the path to be absent, a string requires the current
	 * revision to equal it, and `undefined` skips the check.
	 */
	expectedRevision?: string | null;
}

/** Input for {@link BrowserSyncStorage.move}. */
export interface SyncStorageMoveInput {
	from: string;
	to: string;
	/** Expected revision of the source; `null` requires it to be absent. */
	expectedRevision?: string | null;
}

/** Input for {@link BrowserSyncStorage.delete}. */
export interface SyncStorageDeleteInput {
	path: string;
	/** Expected revision; `null` requires the path to be absent. */
	expectedRevision?: string | null;
}

/**
 * The local replica boundary. `BrowserWorkspaceStorage` provides compatible
 * read/write/move/delete operations; see {@link createBrowserStorageAdapter}.
 */
export interface BrowserSyncStorage {
	read(path: string): Promise<SyncStorageFile | null>;
	write(input: SyncStorageWriteInput): Promise<{ revision: string }>;
	move(input: SyncStorageMoveInput): Promise<void>;
	delete(input: SyncStorageDeleteInput): Promise<void>;
	list(): Promise<string[]>;
}

/**
 * The encrypted sync service boundary. Shapes match `SyncPage` and
 * `SequencedOperation` from `@noura/shared`; cursors and sequences stay
 * canonical decimal strings.
 */
export interface BrowserSyncRemote {
	push(operations: EncryptedOperation[]): Promise<{ sequences: string[] }>;
	pull(cursor: string): Promise<SyncPage>;
}

/**
 * One version-1 file change. `content` is the complete file bytes or `null`
 * for a deletion; `baseRevision` is the revision of the bytes the change is
 * based on, or `null` when the path must be absent.
 */
export interface FileChange {
	path: string;
	previousPath: string | null;
	baseRevision: string | null;
	content: Uint8Array | null;
}

/** An opened file change with the envelope identity recovered by the codec. */
export interface OpenedFileChange extends FileChange {
	workspaceId: string;
	objectId: string;
	epoch: number;
	/**
	 * Present when the payload was version 3: a signed, encrypted attachment
	 * descriptor. `content` is then `null`, and the plaintext must be fetched
	 * through the injected {@link AttachmentFetcher}.
	 */
	blob?: FileChangeBlob;
}

/**
 * Injected boundary that downloads and decrypts a version-3 attachment. The
 * engine only materializes the returned plaintext; it performs no cryptography
 * and never contacts the network itself.
 *
 * A missing, unavailable, or unauthenticated attachment MUST throw so the
 * operation is never applied as an empty file and the cursor never advances.
 */
export interface AttachmentFetcher {
	/** Fetch the decrypted plaintext for one version-3 change. */
	fetch(change: OpenedFileChange): Promise<Uint8Array>;
}

/**
 * Encrypted operation boundary. The codec decides keys, signing, and
 * verification; it must throw an error with code `"revoked"` (or
 * {@link BrowserSyncEngineErrorCode.Revoked}) for an untrusted signer or an
 * undecryptable operation that indicates revocation.
 */
export interface FileChangeCodec {
	sealFileChange(input: FileChange): Promise<EncryptedOperation>;
	openFileChange(operation: EncryptedOperation): Promise<OpenedFileChange>;
}

/** Why a remote operation could not be applied to the local replica. */
export type SyncConflictReason =
	| 'revision_mismatch'
	| 'missing_expected_file'
	| 'unexpected_file'
	| 'occupied_destination'
	| 'missing_move_source'
	| 'invalid_move'
	| 'storage_rejected';

/**
 * A durable record of one remote operation that could not be applied.
 *
 * The encrypted `operation` and its `objectId` are retained so the conflict can
 * be retried later through the codec without re-pulling. Nothing here is
 * plaintext: `operation` is exactly the ciphertext envelope the remote served.
 */
export interface SyncConflict {
	operationId: string;
	/** Envelope object identity, recovered from the operation when it was pulled. */
	objectId: string;
	path: string;
	reason: SyncConflictReason;
	expectedRevision: string | null;
	currentRevision: string | null;
	previousPath: string | null;
	detectedAt: number;
	/** The encrypted remote operation, kept so remote resolution can retry it. */
	operation: EncryptedOperation;
}

/**
 * Durable engine state.
 *
 * `pushedRevisions` maps each synchronized path to its last known content
 * revision; `knownPaths` is the set of synchronized present paths. Together
 * they are the baseline {@link BrowserSyncEngine.snapshotLocalChanges} diffs
 * the local replica against. `outbox` preserves operation order. `version` is
 * the durable state schema version, used to migrate older persisted states.
 */
export interface SyncState {
	version: number;
	cursor: string;
	pushedRevisions: Record<string, string>;
	knownPaths: string[];
	outbox: EncryptedOperation[];
	conflicts: SyncConflict[];
}

/** The `SyncState` schema version written by this package. */
export const SYNC_STATE_VERSION = 2;

/** Reported when an older durable state is read and normalized. */
export interface SyncStateMigration {
	/** Schema version detected on disk; `1` when the field was absent. */
	fromVersion: number;
	/** Schema version after normalization. */
	toVersion: number;
	/** Unresolvable legacy conflict records that were dropped. */
	droppedConflicts: number;
}

/** Options for {@link validateSyncState}. */
export interface ValidateSyncStateOptions {
	/** Called once when legacy state is migrated or unreadable conflicts are dropped. */
	onMigration?: (migration: SyncStateMigration) => void;
}

/** Durable state boundary. Must survive reloads; never a disposable cache. */
export interface SyncStateStore {
	read(): Promise<SyncState>;
	write(state: SyncState): Promise<void>;
}

/** Summary returned by {@link BrowserSyncEngine.reconcile}. */
export interface ReconcileResult {
	/** Operations acknowledged by the remote during this reconcile. */
	pushed: number;
	/** Remote operations applied to the local replica during this reconcile. */
	applied: number;
	/** Conflicts recorded during this reconcile; bytes were left untouched. */
	conflicts: SyncConflict[];
	/** Durable cursor after this reconcile. */
	cursor: string;
	/** Whether the remote reported more operations after the final page. */
	hasMore: boolean;
}

/** Which side wins when resolving a recorded conflict. */
export type SyncConflictResolution = 'local' | 'remote';

/** Summary returned by {@link BrowserSyncEngine.resolveConflict}. */
export interface ResolveConflictResult {
	/** The conflict record that was resolved and removed. */
	resolved: SyncConflict;
	/** Conflicts that remain after resolution, in durable order. */
	remaining: SyncConflict[];
}

/** Engine lifecycle phase. A locked engine refuses further reconciliation. */
export type BrowserSyncEnginePhase = 'active' | 'locked';
