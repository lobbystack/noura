import type {
	ParsedMarkdown,
	WorkspaceFormat,
} from '@noura/workspace-format-wasm';

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder('utf-8', { fatal: true });
const INTERNAL_DIRECTORY = '.noura/browser-storage';
const JOURNAL_DIRECTORY = `${INTERNAL_DIRECTORY}/journals`;
const STAGING_DIRECTORY = `${INTERNAL_DIRECTORY}/staged`;
const IMPORT_DIRECTORY = `${INTERNAL_DIRECTORY}/imports`;
const DEFAULT_LOCK_NAME = 'noura:browser-workspace-storage';

/** Bounds untrusted import snapshots before they can consume OPFS space. */
export const BROWSER_SNAPSHOT_LIMITS = {
	maxEntries: 10_000,
	maxEntryBytes: 64 * 1024 * 1024,
	maxTotalBytes: 512 * 1024 * 1024,
} as const;

/** Limits for an in-memory snapshot that will cross the worker/UI boundary. */
export const BROWSER_EXPORT_SNAPSHOT_LIMITS = {
	maxEntries: BROWSER_SNAPSHOT_LIMITS.maxEntries,
	maxEntryBytes: 32 * 1024 * 1024,
	maxTotalBytes: 64 * 1024 * 1024,
} as const;

export type BrowserWorkspaceSnapshotLimits = {
	maxEntries: number;
	maxEntryBytes: number;
	maxTotalBytes: number;
};

export type DirectoryEntry = {
	name: string;
	kind: 'file' | 'directory';
};

/**
 * The deliberately small filesystem boundary used by the storage adapter.
 * Its paths have already passed `validateRelativePath` when supplied by callers.
 */
export interface BrowserStorageFileSystem {
	/** Rejects before allocating bytes when the file exceeds `maxBytes`. */
	read(path: string, maxBytes?: number): Promise<Uint8Array | null>;
	write(path: string, bytes: Uint8Array): Promise<void>;
	remove(path: string): Promise<boolean>;
	list(path: string): Promise<DirectoryEntry[]>;
}

/** Serializes adapter operations across cooperating browser contexts. */
export interface BrowserStorageLock {
	run<T>(operation: () => Promise<T>): Promise<T>;
}

export type BrowserWorkspaceStorageOptions = {
	fileSystem: BrowserStorageFileSystem;
	format: WorkspaceFormat;
	lock: BrowserStorageLock;
};

export type StoredFile = {
	path: string;
	bytes: Uint8Array;
	revision: string;
};

/** Raw workspace bytes suitable for structured cloning to a browser worker. */
export type BrowserWorkspaceSnapshotEntry = {
	path: string;
	bytes: Uint8Array;
};

export type RebuiltManagedFile = Extract<ParsedMarkdown, { kind: 'managed' }>;

export type RebuildResult = {
	files: Array<{ path: string; revision: string }>;
	managed: RebuiltManagedFile[];
	malformedMarkdown: Array<{ path: string; error: string }>;
};

export class BrowserStorageError extends Error {
	constructor(
		public readonly code:
			| 'invalid_path'
			| 'stale_revision'
			| 'not_found'
			| 'unsupported'
			| 'invalid_snapshot'
			| 'snapshot_too_large'
			| 'workspace_not_empty'
			| 'path_exists'
			| 'recovery_failed',
		message: string,
	) {
		super(message);
		this.name = 'BrowserStorageError';
	}
}

export class DuplicateObjectIdentityError extends BrowserStorageError {
	constructor(
		public readonly id: string,
		public readonly paths: string[],
	) {
		super(
			'recovery_failed',
			`The stable object ID ${id} occurs in more than one canonical file`,
		);
		this.name = 'DuplicateObjectIdentityError';
	}
}

type Journal =
	| { version: 1; id: string; kind: 'write'; path: string }
	| { version: 1; id: string; kind: 'move'; from: string; to: string }
	| { version: 1; id: string; kind: 'delete'; path: string }
	| {
			version: 1;
			id: string;
			kind: 'import';
			entries: Array<{ path: string; index: number }>;
	  };

/**
 * Validates paths against the portable workspace subset, not the host OS.
 * In particular, it rejects Windows-only reserved spellings so an OPFS
 * workspace can later be moved to a native workspace without reinterpretation.
 */
export function validateRelativePath(path: string): string {
	if (
		path.length === 0 ||
		textEncoder.encode(path).byteLength > 4096 ||
		path.startsWith('/') ||
		path.includes('\\') ||
		hasUnpairedSurrogate(path) ||
		[...path].some(
			(character) => character <= '\u001f' || '\\:<>"|?*'.includes(character),
		)
	) {
		throw new BrowserStorageError('invalid_path', 'The path is not portable');
	}

	for (const part of path.split('/')) {
		const stem = part.split('.')[0]?.trim().replace(/ +$/u, '').toUpperCase();
		if (
			part.length === 0 ||
			part === '.' ||
			part === '..' ||
			part.endsWith('.') ||
			part.endsWith(' ') ||
			stem === undefined ||
			isWindowsDeviceName(stem)
		) {
			throw new BrowserStorageError('invalid_path', 'The path is not portable');
		}
	}

	if (
		path === INTERNAL_DIRECTORY ||
		path.startsWith(`${INTERNAL_DIRECTORY}/`)
	) {
		throw new BrowserStorageError(
			'invalid_path',
			'The path is reserved for browser storage recovery data',
		);
	}
	return path;
}

function hasUnpairedSurrogate(value: string): boolean {
	for (let index = 0; index < value.length; index += 1) {
		const code = value.charCodeAt(index);
		if (code >= 0xd800 && code <= 0xdbff) {
			const next = value.charCodeAt(index + 1);
			if (next < 0xdc00 || next > 0xdfff) return true;
			index += 1;
		} else if (code >= 0xdc00 && code <= 0xdfff) {
			return true;
		}
	}
	return false;
}

function isWindowsDeviceName(stem: string): boolean {
	return (
		stem === 'CON' ||
		stem === 'CONIN$' ||
		stem === 'CONOUT$' ||
		stem === 'PRN' ||
		stem === 'AUX' ||
		stem === 'NUL' ||
		/^(COM|LPT)[1-9¹²³]$/u.test(stem)
	);
}

/**
 * Stores canonical workspace bytes in OPFS without interpreting or serializing
 * the workspace format in TypeScript. Mutations are journaled and recoverable;
 * success means the journal has been removed after its canonical file change.
 */
export class BrowserWorkspaceStorage {
	readonly #fileSystem: BrowserStorageFileSystem;
	readonly #format: WorkspaceFormat;
	readonly #lock: BrowserStorageLock;

	constructor({ fileSystem, format, lock }: BrowserWorkspaceStorageOptions) {
		this.#fileSystem = fileSystem;
		this.#format = format;
		this.#lock = lock;
	}

	async recover(): Promise<void> {
		await this.#lock.run(() => this.#recoverLocked());
	}

	async read(path: string): Promise<StoredFile | null> {
		path = validateRelativePath(path);
		return this.#lock.run(async () => {
			await this.#recoverLocked();
			const bytes = await this.#fileSystem.read(path);
			return bytes === null ? null : this.#storedFile(path, bytes);
		});
	}

	async write(input: {
		path: string;
		bytes: Uint8Array;
		expectedRevision: string | null;
	}): Promise<StoredFile> {
		const path = validateRelativePath(input.path);
		const bytes = input.bytes.slice();
		return this.#lock.run(async () => {
			await this.#recoverLocked();
			await this.#assertExpected(path, input.expectedRevision);
			const journal: Journal = {
				version: 1,
				id: transactionId(),
				kind: 'write',
				path,
			};
			await this.#stageAndCommit(journal, bytes);
			return this.#storedFile(path, bytes);
		});
	}

	async move(input: {
		from: string;
		to: string;
		expectedRevision: string;
		expectedDestinationRevision: string | null;
	}): Promise<StoredFile> {
		const from = validateRelativePath(input.from);
		const to = validateRelativePath(input.to);
		if (from === to)
			throw new BrowserStorageError(
				'invalid_path',
				'A file cannot move onto itself',
			);

		return this.#lock.run(async () => {
			await this.#recoverLocked();
			const source = await this.#assertExpected(from, input.expectedRevision);
			if (input.expectedDestinationRevision === null)
				await this.#assertAbsent(to);
			else await this.#assertExpected(to, input.expectedDestinationRevision);
			const journal: Journal = {
				version: 1,
				id: transactionId(),
				kind: 'move',
				from,
				to,
			};
			await this.#stageAndCommit(journal, source.bytes);
			return this.#storedFile(to, source.bytes);
		});
	}

	async delete(input: {
		path: string;
		expectedRevision: string;
	}): Promise<void> {
		const path = validateRelativePath(input.path);
		await this.#lock.run(async () => {
			await this.#recoverLocked();
			await this.#assertExpected(path, input.expectedRevision);
			const journal: Journal = {
				version: 1,
				id: transactionId(),
				kind: 'delete',
				path,
			};
			await this.#writeJournal(journal);
			await this.#applyJournal(journal);
		});
	}

	/**
	 * Returns every durable workspace file verbatim, including ordinary `.noura`
	 * metadata. Adapter journals, staging, and derived indexes are deliberately
	 * excluded because they can be rebuilt locally and may contain transient state.
	 */
	async exportSnapshotEntries(
		limits: BrowserWorkspaceSnapshotLimits = BROWSER_SNAPSHOT_LIMITS,
	): Promise<BrowserWorkspaceSnapshotEntry[]> {
		validateSnapshotLimits(limits);
		return this.#lock.run(async () => {
			await this.#recoverLocked();
			const paths = await this.#enumerateExportedFiles('');
			if (paths.length > limits.maxEntries) throw snapshotTooLarge();
			const entries: BrowserWorkspaceSnapshotEntry[] = [];
			let totalBytes = 0;
			for (const path of paths) {
				const maxBytes = Math.min(
					limits.maxEntryBytes,
					limits.maxTotalBytes - totalBytes,
				);
				const bytes = await this.#fileSystem.read(path, maxBytes);
				if (bytes === null) continue;
				if (bytes.byteLength > maxBytes) throw snapshotTooLarge();
				totalBytes += bytes.byteLength;
				entries.push({ path, bytes });
			}
			return entries;
		});
	}

	/**
	 * Installs a validated snapshot into a fresh workspace. All source bytes are
	 * staged before the import journal is acknowledged, so recovery can complete a
	 * partially applied import after a worker or browser interruption.
	 */
	async importSnapshotEntries(
		entries: readonly BrowserWorkspaceSnapshotEntry[],
	): Promise<void> {
		const copied = validateSnapshotEntries(entries);
		await this.#lock.run(async () => {
			await this.#recoverLocked();
			if ((await this.#enumerateExportedFiles('')).length > 0)
				throw new BrowserStorageError(
					'workspace_not_empty',
					'An imported browser workspace must be created in an empty destination',
				);

			const journal: Extract<Journal, { kind: 'import' }> = {
				version: 1,
				id: transactionId(),
				kind: 'import',
				entries: copied.map((entry, index) => ({ path: entry.path, index })),
			};
			for (const entry of journal.entries)
				await this.#fileSystem.write(
					importStagePath(journal.id, entry.index),
					copied[entry.index]!.bytes,
				);
			await this.#writeJournal(journal);
			await this.#applyJournal(journal);
		});
	}

	/** Re-enumerates canonical files; no database or in-memory index is consulted. */
	async rebuild(): Promise<RebuildResult> {
		return this.#lock.run(async () => {
			await this.#recoverLocked();
			const paths = await this.#enumerateFiles('');
			const files: RebuildResult['files'] = [];
			const managed: RebuiltManagedFile[] = [];
			const malformedMarkdown: RebuildResult['malformedMarkdown'] = [];
			const identities = new Map<string, string[]>();

			for (const path of paths) {
				const bytes = await this.#fileSystem.read(path);
				if (bytes === null) continue;
				files.push({ path, revision: this.#format.contentRevision(bytes) });
				if (!isLiveWorkspaceObjectPath(path) || !path.endsWith('.md')) continue;
				const parsed = this.#format.parseMarkdown(path, bytes);
				if (parsed.kind === 'malformed') {
					malformedMarkdown.push({ path, error: parsed.error });
					continue;
				}
				if (parsed.kind !== 'managed') continue;
				managed.push(parsed);
				const pathsForId = identities.get(parsed.id) ?? [];
				pathsForId.push(path);
				identities.set(parsed.id, pathsForId);
			}

			for (const [id, pathsForId] of identities) {
				if (pathsForId.length > 1)
					throw new DuplicateObjectIdentityError(id, pathsForId);
			}
			return { files, managed, malformedMarkdown };
		});
	}

	async #assertExpected(
		path: string,
		expectedRevision: string | null,
	): Promise<StoredFile> {
		const bytes = await this.#fileSystem.read(path);
		if (bytes === null) {
			if (expectedRevision === null)
				return { path, bytes: new Uint8Array(), revision: '' };
			throw new BrowserStorageError(
				'not_found',
				'The expected file does not exist',
			);
		}
		const current = this.#storedFile(path, bytes);
		if (expectedRevision === null || current.revision !== expectedRevision) {
			throw new BrowserStorageError(
				'stale_revision',
				'The file changed before the mutation could be committed',
			);
		}
		return current;
	}

	async #assertAbsent(path: string): Promise<void> {
		if ((await this.#fileSystem.read(path)) !== null)
			throw new BrowserStorageError(
				'path_exists',
				'A file already exists at the destination',
			);
	}

	#storedFile(path: string, bytes: Uint8Array): StoredFile {
		return { path, bytes, revision: this.#format.contentRevision(bytes) };
	}

	async #stageAndCommit(
		journal: Exclude<Journal, { kind: 'delete' }>,
		bytes: Uint8Array,
	) {
		await this.#fileSystem.write(stagePath(journal.id), bytes);
		await this.#writeJournal(journal);
		await this.#applyJournal(journal);
	}

	async #writeJournal(journal: Journal): Promise<void> {
		await this.#fileSystem.write(
			journalPath(journal.id),
			textEncoder.encode(JSON.stringify(journal)),
		);
	}

	async #applyJournal(journal: Journal): Promise<void> {
		if (journal.kind === 'import') {
			for (const entry of journal.entries) {
				const bytes = await this.#fileSystem.read(
					importStagePath(journal.id, entry.index),
				);
				if (bytes === null)
					throw new BrowserStorageError(
						'recovery_failed',
						`The staged import bytes for ${journal.id} are missing`,
					);
				await this.#fileSystem.write(entry.path, bytes);
			}
			await this.#fileSystem.remove(journalPath(journal.id));
			for (const entry of journal.entries)
				void this.#fileSystem
					.remove(importStagePath(journal.id, entry.index))
					.catch(() => {});
			return;
		}
		if (journal.kind === 'write' || journal.kind === 'move') {
			const bytes = await this.#fileSystem.read(stagePath(journal.id));
			if (bytes === null)
				throw new BrowserStorageError(
					'recovery_failed',
					`The staged bytes for ${journal.id} are missing`,
				);
			await this.#fileSystem.write(
				journal.kind === 'write' ? journal.path : journal.to,
				bytes,
			);
			if (journal.kind === 'move') await this.#fileSystem.remove(journal.from);
		} else {
			await this.#fileSystem.remove(journal.path);
		}

		// Journal removal is the commit acknowledgement. Staging cleanup is best effort.
		await this.#fileSystem.remove(journalPath(journal.id));
		if (journal.kind !== 'delete')
			void this.#fileSystem.remove(stagePath(journal.id)).catch(() => {});
	}

	async #recoverLocked(): Promise<void> {
		const entries = await this.#fileSystem.list(JOURNAL_DIRECTORY);
		for (const entry of entries.sort((left, right) =>
			left.name.localeCompare(right.name),
		)) {
			if (entry.kind !== 'file' || !entry.name.endsWith('.json')) continue;
			const id = entry.name.slice(0, -'.json'.length);
			const bytes = await this.#fileSystem.read(
				`${JOURNAL_DIRECTORY}/${entry.name}`,
			);
			if (bytes === null) continue;
			await this.#applyJournal(parseJournal(id, bytes));
		}
	}

	async #enumerateFiles(directory: string): Promise<string[]> {
		const entries = await this.#fileSystem.list(directory);
		const paths: string[] = [];
		for (const entry of entries.sort((left, right) =>
			left.name.localeCompare(right.name),
		)) {
			const path =
				directory.length === 0 ? entry.name : `${directory}/${entry.name}`;
			if (isExcludedFromSnapshot(path)) continue;
			if (entry.kind === 'file') {
				validateRelativePath(path);
				paths.push(path);
			} else paths.push(...(await this.#enumerateFiles(path)));
		}
		return paths;
	}

	async #enumerateExportedFiles(directory: string): Promise<string[]> {
		const entries = await this.#fileSystem.list(directory);
		const paths: string[] = [];
		for (const entry of entries.sort((left, right) =>
			left.name.localeCompare(right.name),
		)) {
			const path =
				directory.length === 0 ? entry.name : `${directory}/${entry.name}`;
			if (isExcludedFromSnapshot(path)) continue;
			if (entry.kind === 'file') {
				validateRelativePath(path);
				paths.push(path);
			} else {
				paths.push(...(await this.#enumerateExportedFiles(path)));
			}
		}
		return paths;
	}
}

function transactionId(): string {
	return globalThis.crypto.randomUUID();
}

function journalPath(id: string): string {
	return `${JOURNAL_DIRECTORY}/${id}.json`;
}

function stagePath(id: string): string {
	return `${STAGING_DIRECTORY}/${id}.bin`;
}

function importStagePath(id: string, index: number): string {
	return `${IMPORT_DIRECTORY}/${id}/${index}.bin`;
}

function parseJournal(id: string, bytes: Uint8Array): Journal {
	if (!isTransactionId(id)) return malformedJournal();
	let value: unknown;
	try {
		value = JSON.parse(textDecoder.decode(bytes));
	} catch {
		return malformedJournal();
	}
	if (!isRecord(value) || value.version !== 1 || value.id !== id) {
		return malformedJournal();
	}
	try {
		if (
			value.kind === 'write' &&
			typeof value.path === 'string' &&
			hasOnlyKeys(value, ['version', 'id', 'kind', 'path'])
		)
			return {
				version: 1,
				id,
				kind: 'write',
				path: validateRelativePath(value.path),
			};
		if (
			value.kind === 'import' &&
			Array.isArray(value.entries) &&
			hasOnlyKeys(value, ['version', 'id', 'kind', 'entries'])
		) {
			if (
				value.entries.length === 0 ||
				value.entries.length > BROWSER_SNAPSHOT_LIMITS.maxEntries
			)
				return malformedJournal();
			const paths = new Set<string>();
			const entries = value.entries.map((entry, index) => {
				if (
					!isRecord(entry) ||
					typeof entry.path !== 'string' ||
					entry.index !== index ||
					!hasOnlyKeys(entry, ['path', 'index'])
				)
					return malformedJournal();
				const path = validateRelativePath(entry.path);
				if (isExcludedFromSnapshot(path) || paths.has(path))
					return malformedJournal();
				paths.add(path);
				return { path, index };
			});
			return { version: 1, id, kind: 'import', entries };
		}
		if (
			value.kind === 'move' &&
			typeof value.from === 'string' &&
			typeof value.to === 'string' &&
			value.from !== value.to &&
			hasOnlyKeys(value, ['version', 'id', 'kind', 'from', 'to'])
		)
			return {
				version: 1,
				id,
				kind: 'move',
				from: validateRelativePath(value.from),
				to: validateRelativePath(value.to),
			};
		if (
			value.kind === 'delete' &&
			typeof value.path === 'string' &&
			hasOnlyKeys(value, ['version', 'id', 'kind', 'path'])
		)
			return {
				version: 1,
				id,
				kind: 'delete',
				path: validateRelativePath(value.path),
			};
	} catch (error) {
		if (error instanceof BrowserStorageError) return malformedJournal();
		throw error;
	}
	return malformedJournal();
}

function malformedJournal(): never {
	throw new BrowserStorageError(
		'recovery_failed',
		'A recovery journal is malformed',
	);
}

function isTransactionId(value: string): boolean {
	return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu.test(
		value,
	);
}

function hasOnlyKeys(
	value: Record<string, unknown>,
	keys: readonly string[],
): boolean {
	return Object.keys(value).every((key) => keys.includes(key));
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null;
}

function validateSnapshotEntries(
	entries: readonly BrowserWorkspaceSnapshotEntry[],
): BrowserWorkspaceSnapshotEntry[] {
	if (
		entries.length === 0 ||
		entries.length > BROWSER_SNAPSHOT_LIMITS.maxEntries
	)
		throw new BrowserStorageError(
			'invalid_snapshot',
			'The workspace snapshot has an invalid number of files',
		);
	const paths = new Set<string>();
	let totalBytes = 0;
	return entries.map((entry) => {
		if (
			!entry ||
			typeof entry.path !== 'string' ||
			!(entry.bytes instanceof Uint8Array)
		)
			throw new BrowserStorageError(
				'invalid_snapshot',
				'The workspace snapshot contains an invalid file entry',
			);
		const path = validateRelativePath(entry.path);
		if (isExcludedFromSnapshot(path) || paths.has(path))
			throw new BrowserStorageError(
				'invalid_snapshot',
				'The workspace snapshot contains a duplicate or internal path',
			);
		paths.add(path);
		if (entry.bytes.byteLength > BROWSER_SNAPSHOT_LIMITS.maxEntryBytes)
			throw new BrowserStorageError(
				'invalid_snapshot',
				'The workspace snapshot contains a file that is too large',
			);
		totalBytes += entry.bytes.byteLength;
		if (totalBytes > BROWSER_SNAPSHOT_LIMITS.maxTotalBytes)
			throw new BrowserStorageError(
				'invalid_snapshot',
				'The workspace snapshot is too large',
			);
		return { path, bytes: entry.bytes.slice() };
	});
}

export function isExcludedFromSnapshot(path: string): boolean {
	return (
		path === INTERNAL_DIRECTORY ||
		path.startsWith(`${INTERNAL_DIRECTORY}/`) ||
		path === '.noura/index.sqlite' ||
		path === '.noura/index.sqlite-shm' ||
		path === '.noura/index.sqlite-wal'
	);
}

/** Durable metadata is retained, but only ordinary workspace files define live objects. */
export function isLiveWorkspaceObjectPath(path: string): boolean {
	return !path.startsWith('.noura/');
}

function validateSnapshotLimits(limits: BrowserWorkspaceSnapshotLimits): void {
	if (
		!Number.isSafeInteger(limits.maxEntries) ||
		!Number.isSafeInteger(limits.maxEntryBytes) ||
		!Number.isSafeInteger(limits.maxTotalBytes) ||
		limits.maxEntries < 1 ||
		limits.maxEntryBytes < 0 ||
		limits.maxTotalBytes < 0
	)
		throw new BrowserStorageError(
			'invalid_snapshot',
			'The workspace snapshot limits are invalid',
		);
}

function snapshotTooLarge(): BrowserStorageError {
	return new BrowserStorageError(
		'snapshot_too_large',
		'The workspace snapshot exceeds the configured export limits',
	);
}

/** OPFS implementation of the injected filesystem boundary. */
export class OpfsFileSystem implements BrowserStorageFileSystem {
	constructor(private readonly root: FileSystemDirectoryHandle) {}

	async read(path: string, maxBytes?: number): Promise<Uint8Array | null> {
		try {
			const parent = await this.#directory(parentPath(path), false);
			const file = await parent.getFileHandle(fileName(path));
			const snapshot = await file.getFile();
			if (maxBytes !== undefined && snapshot.size > maxBytes)
				throw snapshotTooLarge();
			return new Uint8Array(await snapshot.arrayBuffer());
		} catch (error) {
			if (isNotFound(error)) return null;
			throw error;
		}
	}

	async write(path: string, bytes: Uint8Array): Promise<void> {
		const parent = await this.#directory(parentPath(path), true);
		const file = await parent.getFileHandle(fileName(path), { create: true });
		const writable = await file.createWritable();
		try {
			await writable.write(
				bytes.buffer.slice(
					bytes.byteOffset,
					bytes.byteOffset + bytes.byteLength,
				) as ArrayBuffer,
			);
			await writable.close();
		} catch (error) {
			await writable.abort().catch(() => {});
			throw error;
		}
	}

	async remove(path: string): Promise<boolean> {
		try {
			const parent = await this.#directory(parentPath(path), false);
			await parent.removeEntry(fileName(path));
			return true;
		} catch (error) {
			if (isNotFound(error)) return false;
			throw error;
		}
	}

	async list(path: string): Promise<DirectoryEntry[]> {
		try {
			const directory = await this.#directory(path, false);
			const entries: DirectoryEntry[] = [];
			for await (const handle of (
				directory as FileSystemDirectoryHandle & {
					values(): AsyncIterable<FileSystemHandle>;
				}
			).values()) {
				entries.push({ name: handle.name, kind: handle.kind });
			}
			return entries;
		} catch (error) {
			if (isNotFound(error)) return [];
			throw error;
		}
	}

	async #directory(
		path: string,
		create: boolean,
	): Promise<FileSystemDirectoryHandle> {
		let directory = this.root;
		if (path.length === 0) return directory;
		for (const part of path.split('/')) {
			directory = await directory.getDirectoryHandle(part, { create });
		}
		return directory;
	}
}

function parentPath(path: string): string {
	const separator = path.lastIndexOf('/');
	return separator === -1 ? '' : path.slice(0, separator);
}

function fileName(path: string): string {
	return path.slice(path.lastIndexOf('/') + 1);
}

function isNotFound(error: unknown): boolean {
	return error instanceof DOMException && error.name === 'NotFoundError';
}

/** Creates a Web Locks-backed exclusive lock. OPFS requires this for supported multi-tab use. */
export function createBrowserStorageLock(
	name = DEFAULT_LOCK_NAME,
): BrowserStorageLock {
	const locks = (
		globalThis.navigator as
			| (Navigator & {
					locks?: {
						request<T>(
							name: string,
							options: { mode: 'exclusive' },
							callback: () => T | PromiseLike<T>,
						): Promise<T>;
					};
			  })
			| undefined
	)?.locks;
	if (locks === undefined)
		throw new BrowserStorageError(
			'unsupported',
			'This browser does not provide the Web Locks API required for OPFS mutations',
		);
	return {
		async run<T>(operation: () => Promise<T>): Promise<T> {
			return locks.request<T>(name, { mode: 'exclusive' }, operation);
		},
	};
}

/**
 * Opens origin-private storage for use from a browser worker. Recovery completes
 * before this promise resolves, so callers can rebuild directly from canonical files.
 */
export async function openBrowserWorkspaceStorage(
	format: WorkspaceFormat,
	options: { lockName?: string } = {},
): Promise<BrowserWorkspaceStorage> {
	const storage = globalThis.navigator?.storage;
	if (storage === undefined || typeof storage.getDirectory !== 'function')
		throw new BrowserStorageError(
			'unsupported',
			'This browser does not provide the Origin Private File System',
		);
	const adapter = new BrowserWorkspaceStorage({
		fileSystem: new OpfsFileSystem(await storage.getDirectory()),
		format,
		lock: createBrowserStorageLock(options.lockName),
	});
	await adapter.recover();
	return adapter;
}
