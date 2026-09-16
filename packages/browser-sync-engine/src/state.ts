/**
 * Durable sync state helpers and stores.
 *
 * {@link createFileSystemSyncStateStore} persists state through an injected
 * filesystem boundary at an adapter-owned path. That path must stay outside the
 * canonical workspace and its backups: this file is adapter state, never a
 * workspace file, and it is never the canonical copy of anything. The in-memory
 * store is not durable and is intended for tests only.
 */
import type { EncryptedOperation } from '@noura/shared';
import { BrowserSyncEngineError, BrowserSyncEngineErrorCode } from './errors';
import { SYNC_STATE_VERSION } from './types';
import type {
	SyncConflict,
	SyncState,
	SyncStateMigration,
	SyncStateStore,
	ValidateSyncStateOptions,
} from './types';

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder('utf-8', { fatal: true });

/** Default adapter-owned state location, outside canonical workspace files. */
export const DEFAULT_SYNC_STATE_PATH = '.noura-adapter/sync-state.json';

/**
 * Upper bound on a durable state file. A state file that exceeds it is treated as
 * corrupt rather than parsed, so a truncated or runaway write cannot exhaust
 * memory before it is reported.
 */
export const MAX_SYNC_STATE_BYTES = 8 * 1024 * 1024;

/**
 * The minimal filesystem boundary the state store needs. It is the subset of
 * `BrowserStorageFileSystem` that omits enumeration; callers must supply a
 * filesystem whose root places {@link DEFAULT_SYNC_STATE_PATH} outside the
 * canonical workspace.
 */
export interface SyncStateFileSystem {
	/** Rejects before allocating bytes when the file exceeds `maxBytes`. */
	read(path: string, maxBytes?: number): Promise<Uint8Array | null>;
	write(path: string, bytes: Uint8Array): Promise<void>;
	remove(path: string): Promise<boolean>;
}

/** A {@link SyncStateStore} that can also delete its backing file. */
export interface FileSystemSyncStateStore extends SyncStateStore {
	/** Deletes the backing file, if present. Returns whether a file was removed. */
	remove(): Promise<boolean>;
}

/** The initial state for a workspace that has never synchronized. */
export function createEmptySyncState(): SyncState {
	return {
		version: SYNC_STATE_VERSION,
		cursor: '0',
		pushedRevisions: {},
		knownPaths: [],
		outbox: [],
		conflicts: [],
	};
}

/** Deep-enough clone so callers never mutate a store's in-flight state. */
export function cloneSyncState(state: SyncState): SyncState {
	return {
		version: state.version,
		cursor: state.cursor,
		pushedRevisions: { ...state.pushedRevisions },
		knownPaths: [...state.knownPaths],
		outbox: [...state.outbox],
		conflicts: state.conflicts.map((conflict) => ({
			...conflict,
			operation: { ...conflict.operation },
		})),
	};
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isNullableString(value: unknown): value is string | null {
	return value === null || typeof value === 'string';
}

/** Lightly validate an envelope so a stored conflict holds an usable operation. */
function isEncryptedOperation(value: unknown): value is EncryptedOperation {
	return (
		isRecord(value) &&
		typeof value.operationId === 'string' &&
		value.operationId.length > 0 &&
		typeof value.workspaceId === 'string' &&
		typeof value.objectId === 'string'
	);
}

/** True when a conflict record can be retried with the stored operation. */
function isResolvableConflict(value: unknown): value is SyncConflict {
	return (
		isRecord(value) &&
		typeof value.operationId === 'string' &&
		value.operationId.length > 0 &&
		typeof value.objectId === 'string' &&
		typeof value.path === 'string' &&
		typeof value.reason === 'string' &&
		isNullableString(value.expectedRevision) &&
		isNullableString(value.currentRevision) &&
		isNullableString(value.previousPath) &&
		typeof value.detectedAt === 'number' &&
		isEncryptedOperation(value.operation)
	);
}

function detectedVersion(value: unknown): number {
	if (value === undefined) return 1;
	if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 1) {
		throw invalidState();
	}
	return value;
}

/**
 * Validate an untrusted state value read from a store.
 *
 * Older state that lacks `version`, or that holds conflict records from before
 * the encrypted operation was retained, is migrated rather than rejected: the
 * state is upgraded to {@link SYNC_STATE_VERSION} and any unresolvable conflict
 * records are dropped and reported through `options.onMigration`. Only a value
 * that cannot be interpreted as sync state at all is rejected with
 * {@link BrowserSyncEngineErrorCode.InvalidState}.
 */
export function validateSyncState(
	value: unknown,
	options: ValidateSyncStateOptions = {},
): SyncState {
	if (!isRecord(value)) throw invalidState();
	if (typeof value.cursor !== 'string') throw invalidState();
	if (!isRecord(value.pushedRevisions)) throw invalidState();
	for (const revision of Object.values(value.pushedRevisions)) {
		if (typeof revision !== 'string') throw invalidState();
	}
	if (
		!Array.isArray(value.knownPaths) ||
		value.knownPaths.some((path) => typeof path !== 'string')
	) {
		throw invalidState();
	}
	if (!Array.isArray(value.outbox)) throw invalidState();
	if (!Array.isArray(value.conflicts)) throw invalidState();

	const fromVersion = detectedVersion(value.version);
	if (fromVersion > SYNC_STATE_VERSION) {
		throw new BrowserSyncEngineError(
			BrowserSyncEngineErrorCode.InvalidState,
			'Durable sync state was written by a newer schema version',
		);
	}

	const conflicts: SyncConflict[] = [];
	let droppedConflicts = 0;
	for (const entry of value.conflicts) {
		if (isResolvableConflict(entry)) {
			conflicts.push(entry);
			continue;
		}
		if (!isRecord(entry)) throw invalidState();
		droppedConflicts += 1;
	}

	if (fromVersion !== SYNC_STATE_VERSION || droppedConflicts > 0) {
		const migration: SyncStateMigration = {
			fromVersion,
			toVersion: SYNC_STATE_VERSION,
			droppedConflicts,
		};
		options.onMigration?.(migration);
	}

	return {
		version: SYNC_STATE_VERSION,
		cursor: value.cursor,
		pushedRevisions: { ...(value.pushedRevisions as Record<string, string>) },
		knownPaths: [...(value.knownPaths as string[])],
		outbox: value.outbox as EncryptedOperation[],
		conflicts,
	};
}

function invalidState(cause?: unknown): BrowserSyncEngineError {
	return new BrowserSyncEngineError(
		BrowserSyncEngineErrorCode.InvalidState,
		'Durable sync state was malformed',
		{ cause },
	);
}

/**
 * Canonicalizes a state value so equal state serializes to equal bytes
 * regardless of object key insertion order. Array order is meaningful (outbox
 * order, conflict order) and is preserved.
 */
function canonicalize(value: unknown): unknown {
	if (Array.isArray(value)) return value.map(canonicalize);
	if (value !== null && typeof value === 'object') {
		const record = value as Record<string, unknown>;
		const sorted: Record<string, unknown> = {};
		for (const key of Object.keys(record).sort()) {
			sorted[key] = canonicalize(record[key]);
		}
		return sorted;
	}
	return value;
}

/** Deterministic UTF-8 serialization of durable sync state. */
export function serializeSyncState(state: SyncState): Uint8Array {
	return textEncoder.encode(JSON.stringify(canonicalize(state)));
}

/** Parse and validate state bytes, reporting malformed input as a typed error. */
function parseSyncState(
	bytes: Uint8Array,
	options: ValidateSyncStateOptions = {},
): SyncState {
	let value: unknown;
	try {
		value = JSON.parse(textDecoder.decode(bytes));
	} catch (cause) {
		throw invalidState(cause);
	}
	return validateSyncState(value, options);
}

/**
 * Durable {@link SyncStateStore} backed by an injected filesystem boundary.
 *
 * `read` returns an empty state when the file is absent and a typed
 * {@link BrowserSyncEngineErrorCode.InvalidState} when the file is present but
 * malformed or oversized; it never silently resets a populated state. `write`
 * refuses to overwrite a malformed file, so corruption is reported rather than
 * erased; call {@link FileSystemSyncStateStore.remove} to discard it. `write`
 * resolves only after the injected `write` resolves.
 */
export function createFileSystemSyncStateStore(
	fileSystem: SyncStateFileSystem,
	path: string = DEFAULT_SYNC_STATE_PATH,
	maxBytes: number = MAX_SYNC_STATE_BYTES,
	options: ValidateSyncStateOptions = {},
): FileSystemSyncStateStore {
	return {
		async read(): Promise<SyncState> {
			const bytes = await fileSystem.read(path, maxBytes);
			if (bytes === null) return createEmptySyncState();
			if (bytes.byteLength > maxBytes) throw invalidState();
			return parseSyncState(bytes, options);
		},
		async write(state: SyncState): Promise<void> {
			const existing = await fileSystem.read(path, maxBytes);
			if (existing !== null) {
				if (existing.byteLength > maxBytes) throw invalidState();
				parseSyncState(existing);
			}
			const serialized = serializeSyncState(state);
			if (serialized.byteLength > maxBytes) throw invalidState();
			await fileSystem.write(path, serialized);
		},
		async remove(): Promise<boolean> {
			return fileSystem.remove(path);
		},
	};
}

/** In-memory {@link SyncStateStore} for tests. Not durable. */
export function createMemorySyncStateStore(
	initial: SyncState = createEmptySyncState(),
): SyncStateStore {
	let state = cloneSyncState(initial);
	return {
		async read(): Promise<SyncState> {
			return cloneSyncState(state);
		},
		async write(next: SyncState): Promise<void> {
			state = cloneSyncState(next);
		},
	};
}
