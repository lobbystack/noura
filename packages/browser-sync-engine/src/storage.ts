/**
 * Local replica storage adapters.
 *
 * {@link MemorySyncStorage} is an in-memory replica for tests. It is not
 * durable. {@link createBrowserStorageAdapter} bridges `BrowserWorkspaceStorage`
 * (which owns OPFS durability and recovery) onto {@link BrowserSyncStorage}
 * without importing it; the host supplies a `list` implementation.
 * {@link createWorkspaceStorageAdapter} is the concrete bridge: it takes only a
 * workspace storage and derives `list` from `rebuild()`, so no separate
 * enumeration is required.
 */
import { BrowserSyncEngineError, BrowserSyncEngineErrorCode } from './errors';
import type {
	BrowserSyncStorage,
	SyncStorageDeleteInput,
	SyncStorageFile,
	SyncStorageMoveInput,
	SyncStorageWriteInput,
} from './types';

/** Storage error raised by the in-memory adapter. */
export class MemorySyncStorageError extends Error {
	readonly code: 'not_found' | 'path_exists' | 'stale_revision';

	constructor(
		code: 'not_found' | 'path_exists' | 'stale_revision',
		message: string,
	) {
		super(message);
		this.name = 'MemorySyncStorageError';
		this.code = code;
	}
}

/**
 * Deterministic opaque revision for in-memory bytes. Real adapters use the
 * workspace format's BLAKE3 content revision; tests only need a stable value.
 */
export function memoryRevision(bytes: Uint8Array): string {
	let hash = 0xcbf29ce484222325n;
	for (const byte of bytes) {
		hash ^= BigInt(byte);
		hash = (hash * 0x100000001b3n) & 0xffffffffffffffffn;
	}
	return hash.toString(16).padStart(16, '0');
}

function assertExpectedRevision(
	path: string,
	expected: string | null | undefined,
	bytes: Uint8Array | undefined,
): void {
	if (expected === undefined) return;
	if (expected === null) {
		if (bytes !== undefined) {
			throw new MemorySyncStorageError(
				'path_exists',
				`Expected ${path} to be absent`,
			);
		}
		return;
	}
	if (bytes === undefined) {
		throw new MemorySyncStorageError('not_found', `Expected ${path} to exist`);
	}
	if (memoryRevision(bytes) !== expected) {
		throw new MemorySyncStorageError(
			'stale_revision',
			`The revision of ${path} did not match`,
		);
	}
}

/** In-memory {@link BrowserSyncStorage}. Not durable; intended for tests. */
export class MemorySyncStorage implements BrowserSyncStorage {
	readonly #files = new Map<string, Uint8Array>();

	constructor(initial: Record<string, Uint8Array> = {}) {
		for (const [path, bytes] of Object.entries(initial)) {
			this.#files.set(path, bytes.slice());
		}
	}

	async read(path: string): Promise<SyncStorageFile | null> {
		const bytes = this.#files.get(path);
		return bytes === undefined
			? null
			: { bytes: bytes.slice(), revision: memoryRevision(bytes) };
	}

	async write(input: SyncStorageWriteInput): Promise<{ revision: string }> {
		const current = this.#files.get(input.path);
		assertExpectedRevision(input.path, input.expectedRevision, current);
		const bytes = input.bytes.slice();
		this.#files.set(input.path, bytes);
		return { revision: memoryRevision(bytes) };
	}

	async move(input: SyncStorageMoveInput): Promise<void> {
		const source = this.#files.get(input.from);
		assertExpectedRevision(input.from, input.expectedRevision, source);
		if (source === undefined) {
			throw new MemorySyncStorageError(
				'not_found',
				`Expected ${input.from} to exist`,
			);
		}
		if (this.#files.has(input.to)) {
			throw new MemorySyncStorageError(
				'path_exists',
				`A file already exists at ${input.to}`,
			);
		}
		this.#files.set(input.to, source.slice());
		this.#files.delete(input.from);
	}

	async delete(input: SyncStorageDeleteInput): Promise<void> {
		const current = this.#files.get(input.path);
		if (current === undefined) {
			if (input.expectedRevision === null) return;
			throw new MemorySyncStorageError(
				'not_found',
				`Expected ${input.path} to exist`,
			);
		}
		if (
			typeof input.expectedRevision === 'string' &&
			memoryRevision(current) !== input.expectedRevision
		) {
			throw new MemorySyncStorageError(
				'stale_revision',
				`The revision of ${input.path} did not match`,
			);
		}
		this.#files.delete(input.path);
	}

	async list(): Promise<string[]> {
		return [...this.#files.keys()].sort();
	}
}

/**
 * The subset of `BrowserWorkspaceStorage` the adapter consumes. Declared
 * structurally so this package does not depend on `@noura/browser-storage`.
 */
export interface BrowserWorkspaceStorageLike {
	read(path: string): Promise<{ bytes: Uint8Array; revision: string } | null>;
	write(input: {
		path: string;
		bytes: Uint8Array;
		expectedRevision: string | null;
	}): Promise<unknown>;
	move(input: {
		from: string;
		to: string;
		expectedRevision: string;
		expectedDestinationRevision: string | null;
	}): Promise<unknown>;
	delete(input: { path: string; expectedRevision: string }): Promise<void>;
}

/**
 * Bridge `BrowserWorkspaceStorage` (or a compatible adapter) onto
 * {@link BrowserSyncStorage}. The caller supplies `list`, since
 * `BrowserWorkspaceStorage` enumerates through `exportSnapshotEntries` rather
 * than exposing a bare path list.
 *
 * Destination-absent checks are performed by the engine before a move, so the
 * bridge always passes `expectedDestinationRevision: null`.
 */
export function createBrowserStorageAdapter(input: {
	storage: BrowserWorkspaceStorageLike;
	list(): Promise<string[]>;
}): BrowserSyncStorage {
	return {
		read(path: string): Promise<SyncStorageFile | null> {
			return input.storage.read(path);
		},
		async write({
			path,
			bytes,
			expectedRevision,
		}: SyncStorageWriteInput): Promise<{ revision: string }> {
			const result = (await input.storage.write({
				path,
				bytes,
				expectedRevision: expectedRevision ?? null,
			})) as { revision?: unknown };
			if (!result || typeof result.revision !== 'string') {
				throw new BrowserSyncEngineError(
					BrowserSyncEngineErrorCode.InvalidResponse,
					'The storage adapter did not return a revision',
				);
			}
			return { revision: result.revision };
		},
		async move({
			from,
			to,
			expectedRevision,
		}: SyncStorageMoveInput): Promise<void> {
			await input.storage.move({
				from,
				to,
				expectedRevision: requireExpectedRevision('move', expectedRevision),
				expectedDestinationRevision: null,
			});
		},
		async delete({
			path,
			expectedRevision,
		}: SyncStorageDeleteInput): Promise<void> {
			await input.storage.delete({
				path,
				expectedRevision: requireExpectedRevision('delete', expectedRevision),
			});
		},
		list(): Promise<string[]> {
			return input.list();
		},
	};
}

/**
 * A workspace storage that can rebuild its file list from canonical files. This
 * is the subset of `BrowserWorkspaceStorage` the concrete bridge consumes;
 * declared structurally so this package does not depend on
 * `@noura/browser-storage`. `managed` entries are reported by `relativePath`
 * (with `path` accepted for compatible adapters), matching
 * `BrowserWorkspaceStorage.rebuild()`.
 */
export interface WorkspaceRebuildResult {
	/** Every durable workspace file, with its content revision. */
	files: ReadonlyArray<{ path: string }>;
	/** Managed markdown objects; the bridge only needs their path. */
	managed: ReadonlyArray<{ path?: string; relativePath?: string }>;
	/** Malformed managed files, reported by `rebuild()` but not synchronized. */
	malformedMarkdown?: ReadonlyArray<{ path: string; error: string }>;
}

/** A stored canonical file as returned by a workspace storage `read`. */
export interface WorkspaceStoredFile {
	bytes: Uint8Array;
	revision: string;
}

/**
 * The subset of `BrowserWorkspaceStorage` the concrete adapter consumes.
 * Structurally compatible with `BrowserWorkspaceStorage`, which owns OPFS
 * durability and recovery; declared here so this package does not depend on
 * `@noura/browser-storage`.
 */
export interface WorkspaceStorageLike {
	read(path: string): Promise<WorkspaceStoredFile | null>;
	write(input: {
		path: string;
		bytes: Uint8Array;
		expectedRevision?: string | null;
	}): Promise<{ revision: string }>;
	move(input: {
		from: string;
		to: string;
		expectedRevision?: string | null;
	}): Promise<unknown>;
	delete(input: {
		path: string;
		expectedRevision?: string | null;
	}): Promise<void>;
	/** Re-enumerates canonical files from durable storage; no cache is consulted. */
	rebuild(): Promise<WorkspaceRebuildResult>;
}

function requireExpectedRevision(
	action: 'move' | 'delete',
	expectedRevision: string | null | undefined,
): string {
	if (typeof expectedRevision !== 'string') {
		throw new BrowserSyncEngineError(
			BrowserSyncEngineErrorCode.InvalidStorageCall,
			action === 'move'
				? 'A move requires the current source revision'
				: 'A delete requires the current revision',
		);
	}
	return expectedRevision;
}

function listWorkspacePaths(result: WorkspaceRebuildResult): string[] {
	const paths = new Set<string>();
	for (const file of result.files ?? []) {
		if (typeof file?.path === 'string') paths.add(file.path);
	}
	for (const managed of result.managed ?? []) {
		const path = managed?.relativePath ?? managed?.path;
		if (typeof path === 'string') paths.add(path);
	}
	return [...paths].sort();
}

/**
 * Bridge a `BrowserWorkspaceStorage`-like replica onto {@link BrowserSyncStorage}
 * without importing `@noura/browser-storage`. Unlike
 * {@link createBrowserStorageAdapter}, the caller supplies no `list`:
 * {@link WorkspaceStorageLike.rebuild} is consulted so the path set always
 * reflects canonical files (ordinary and managed) rather than a stale index.
 *
 * `write` and `move` pass `expectedRevision` through unchanged, so a stale or
 * missing-file rejection from the workspace storage propagates to the engine and
 * becomes a recorded conflict. Destination-absent checks are performed by the
 * engine before a move.
 */
export function createWorkspaceStorageAdapter(
	workspace: WorkspaceStorageLike,
): BrowserSyncStorage {
	return {
		async read(path: string): Promise<SyncStorageFile | null> {
			const file = await workspace.read(path);
			return file === null
				? null
				: { bytes: file.bytes, revision: file.revision };
		},
		async write({
			path,
			bytes,
			expectedRevision,
		}: SyncStorageWriteInput): Promise<{ revision: string }> {
			const result = await workspace.write({
				path,
				bytes,
				expectedRevision: expectedRevision ?? null,
			});
			const revision = (result as { revision?: unknown } | null | undefined)
				?.revision;
			if (typeof revision !== 'string') {
				throw new BrowserSyncEngineError(
					BrowserSyncEngineErrorCode.InvalidResponse,
					'The workspace storage adapter did not return a revision',
				);
			}
			return { revision };
		},
		async move({
			from,
			to,
			expectedRevision,
		}: SyncStorageMoveInput): Promise<void> {
			await workspace.move({
				from,
				to,
				expectedRevision: requireExpectedRevision('move', expectedRevision),
			});
		},
		async delete({
			path,
			expectedRevision,
		}: SyncStorageDeleteInput): Promise<void> {
			await workspace.delete({
				path,
				expectedRevision: requireExpectedRevision('delete', expectedRevision),
			});
		},
		async list(): Promise<string[]> {
			return listWorkspacePaths(await workspace.rebuild());
		},
	};
}
