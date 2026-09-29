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
	/** Creates a folder and its parents. Existing folders are left alone. */
	makeDirectory(path: string): Promise<void>;
	/**
	 * Removes a folder. Without `recursive` it must be empty. Resolves false
	 * when the folder does not exist.
	 */
	removeDirectory(
		path: string,
		options?: { recursive?: boolean },
	): Promise<boolean>;
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

/** One canonical file as the last enumeration or mutation saw it. */
export type RebuiltFile = {
	path: string;
	revision: string;
	/**
	 * The parsed Markdown for a live workspace `.md` file, otherwise null.
	 * Metadata under `.noura` and hidden folders is kept but never parsed.
	 */
	parsed: ParsedMarkdown | null;
};

export type RebuildResult = {
	files: Array<{ path: string; revision: string }>;
	/** Every file with its parse result, sorted by path. */
	entries: RebuiltFile[];
	/** Every folder, including empty ones, sorted by path. */
	folders: string[];
	/** Managed objects, including every file of a duplicated stable ID. */
	managed: RebuiltManagedFile[];
	malformedMarkdown: Array<{ path: string; error: string }>;
	/**
	 * Stable IDs that more than one live file uses. The files stay untouched;
	 * only operations addressed by one of these IDs fail.
	 */
	duplicates: Array<{ id: string; paths: string[] }>;
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
			| 'folder_not_empty'
			| 'recovery_failed',
		message: string,
	) {
		super(message);
		this.name = 'BrowserStorageError';
	}
}

type Journal =
	| { version: 1; id: string; kind: 'write'; path: string }
	| { version: 1; id: string; kind: 'move'; from: string; to: string }
	| { version: 1; id: string; kind: 'delete'; path: string }
	| {
			version: 1;
			id: string;
			kind: 'moves';
			moves: Array<{ from: string; to: string }>;
	  }
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

type CachedFile = { revision: string; parsed: ParsedMarkdown | null };

type WorkspaceCache = {
	files: Map<string, CachedFile>;
	folders: Set<string>;
};

/**
 * Stores canonical workspace bytes in OPFS without interpreting or serializing
 * the workspace format in TypeScript. Mutations are journaled and recoverable;
 * success means the journal has been removed after its canonical file change.
 *
 * The first read enumerates and parses every file once. Later reads reuse that
 * projection, and each mutation updates only the paths it changed. Another
 * browser context that changes the same files must call `invalidate`.
 */
export class BrowserWorkspaceStorage {
	readonly #fileSystem: BrowserStorageFileSystem;
	readonly #format: WorkspaceFormat;
	readonly #lock: BrowserStorageLock;
	#cache: WorkspaceCache | null = null;
	#result: RebuildResult | null = null;

	constructor({ fileSystem, format, lock }: BrowserWorkspaceStorageOptions) {
		this.#fileSystem = fileSystem;
		this.#format = format;
		this.#lock = lock;
	}

	async recover(): Promise<void> {
		await this.#lock.run(() => this.#recoverLocked());
	}

	/**
	 * Forget what this adapter knows about `paths` (or everything) because
	 * another context changed them. The next read enumerates again.
	 */
	async invalidate(paths?: readonly string[]): Promise<void> {
		await this.#lock.run(async () => {
			await this.#recoverLocked();
			if (!this.#cache) return;
			if (paths === undefined) {
				this.#drop();
				return;
			}
			for (const path of paths) {
				let valid: string;
				try {
					valid = validateRelativePath(path);
				} catch {
					continue;
				}
				const bytes = await this.#fileSystem.read(valid);
				if (bytes === null) this.#forget(valid);
				else this.#remember(valid, bytes);
			}
		});
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
			await this.#assertNotFolder(path);
			const journal: Journal = {
				version: 1,
				id: transactionId(),
				kind: 'write',
				path,
			};
			await this.#stageAndCommit(journal, bytes);
			this.#remember(path, bytes);
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
			this.#forget(from);
			this.#remember(to, source.bytes);
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
			this.#forget(path);
		});
	}

	/**
	 * Creates an empty folder. Refuses when a file or folder already uses the
	 * path.
	 */
	async createFolder(path: string): Promise<void> {
		path = validateRelativePath(path);
		await this.#lock.run(async () => {
			await this.#recoverLocked();
			await this.#assertAbsent(path);
			const cache = await this.#loaded();
			if (cache.folders.has(path))
				throw new BrowserStorageError(
					'path_exists',
					'A folder already exists at the destination',
				);
			await this.#fileSystem.makeDirectory(path);
			this.#addFolders(cache, path, true);
			this.#result = null;
		});
	}

	/** Removes an empty folder. */
	async removeEmptyFolder(path: string): Promise<void> {
		path = validateRelativePath(path);
		await this.#lock.run(async () => {
			await this.#recoverLocked();
			const cache = await this.#loaded();
			if (!cache.folders.has(path))
				throw new BrowserStorageError('not_found', 'The folder does not exist');
			if ((await this.#fileSystem.list(path)).length > 0)
				throw new BrowserStorageError(
					'folder_not_empty',
					'The folder is not empty',
				);
			await this.#fileSystem.removeDirectory(path);
			cache.folders.delete(path);
			this.#result = null;
		});
	}

	/**
	 * Moves a file or a whole folder to `to`, which must not exist. A folder
	 * moves file by file under one journal, so an interrupted move finishes on
	 * the next start. Returns the files that moved.
	 */
	async movePath(input: {
		from: string;
		to: string;
	}): Promise<Array<{ from: string; to: string }>> {
		const from = validateRelativePath(input.from);
		const to = validateRelativePath(input.to);
		if (from === to || to.startsWith(`${from}/`))
			throw new BrowserStorageError(
				'invalid_path',
				'A folder cannot move into itself',
			);
		return this.#lock.run(async () => {
			await this.#recoverLocked();
			const cache = await this.#loaded();
			const isFolder = cache.folders.has(from);
			if (!isFolder && !cache.files.has(from))
				throw new BrowserStorageError(
					'not_found',
					'The file or folder does not exist',
				);
			await this.#assertAbsent(to);
			if (cache.folders.has(to))
				throw new BrowserStorageError(
					'path_exists',
					'A folder already exists at the destination',
				);
			const moves = isFolder
				? [...cache.files.keys()]
						.filter((path) => path.startsWith(`${from}/`))
						.sort()
						.map((path) => ({
							from: path,
							to: `${to}${path.slice(from.length)}`,
						}))
				: [{ from, to }];
			for (const move of moves) validateRelativePath(move.to);
			const emptyFolders = isFolder
				? [...cache.folders]
						.filter((path) => path === from || path.startsWith(`${from}/`))
						.map((path) => `${to}${path.slice(from.length)}`)
				: [];
			if (moves.length > 0) {
				const journal: Journal = {
					version: 1,
					id: transactionId(),
					kind: 'moves',
					moves,
				};
				await this.#writeJournal(journal);
				await this.#applyJournal(journal);
			}
			for (const folder of emptyFolders)
				await this.#fileSystem.makeDirectory(folder);
			if (isFolder)
				await this.#fileSystem.removeDirectory(from, { recursive: true });
			for (const move of moves) {
				const moved = cache.files.get(move.from);
				cache.files.delete(move.from);
				if (moved)
					cache.files.set(move.to, {
						revision: moved.revision,
						parsed: this.#parse(
							move.to,
							(await this.#fileSystem.read(move.to)) ?? new Uint8Array(),
						),
					});
				this.#addFolders(cache, parentPath(move.to), true);
			}
			if (isFolder) {
				for (const folder of [...cache.folders])
					if (folder === from || folder.startsWith(`${from}/`))
						cache.folders.delete(folder);
				for (const folder of emptyFolders)
					this.#addFolders(cache, folder, true);
			}
			this.#result = null;
			return moves;
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
			const paths = (await this.#enumerate('')).files;
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
			if ((await this.#enumerate('')).files.length > 0)
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
			this.#drop();
		});
	}

	/**
	 * The workspace as its canonical files describe it. The first call reads
	 * every file; later calls reuse that projection until a mutation or
	 * `invalidate` changes it. No database is consulted.
	 */
	async rebuild(): Promise<RebuildResult> {
		return this.#lock.run(async () => {
			await this.#recoverLocked();
			const cache = await this.#loaded();
			this.#result ??= this.#project(cache);
			return this.#result;
		});
	}

	async #loaded(): Promise<WorkspaceCache> {
		if (this.#cache) return this.#cache;
		const listing = await this.#enumerate('');
		const cache: WorkspaceCache = {
			files: new Map(),
			folders: new Set(listing.folders),
		};
		for (const path of listing.files) {
			const bytes = await this.#fileSystem.read(path);
			if (bytes === null) continue;
			cache.files.set(path, {
				revision: this.#format.contentRevision(bytes),
				parsed: this.#parse(path, bytes),
			});
		}
		this.#cache = cache;
		this.#result = null;
		return cache;
	}

	#project(cache: WorkspaceCache): RebuildResult {
		const entries = [...cache.files]
			.map(([path, file]) => ({ path, ...file }))
			.sort((left, right) => compareCodeUnits(left.path, right.path));
		const managed: RebuiltManagedFile[] = [];
		const malformedMarkdown: RebuildResult['malformedMarkdown'] = [];
		const identities = new Map<string, string[]>();
		for (const entry of entries) {
			if (entry.parsed?.kind === 'malformed')
				malformedMarkdown.push({ path: entry.path, error: entry.parsed.error });
			if (entry.parsed?.kind !== 'managed') continue;
			managed.push(entry.parsed);
			const paths = identities.get(entry.parsed.id) ?? [];
			paths.push(entry.path);
			identities.set(entry.parsed.id, paths);
		}
		return {
			files: entries.map(({ path, revision }) => ({ path, revision })),
			entries,
			folders: [...cache.folders].sort(compareCodeUnits),
			managed,
			malformedMarkdown,
			duplicates: [...identities]
				.filter(([, paths]) => paths.length > 1)
				.map(([id, paths]) => ({ id, paths }))
				.sort((left, right) => compareCodeUnits(left.id, right.id)),
		};
	}

	#parse(path: string, bytes: Uint8Array): ParsedMarkdown | null {
		if (!isLiveWorkspaceObjectPath(path) || !path.endsWith('.md')) return null;
		let parsed: ParsedMarkdown;
		try {
			parsed = this.#format.parseMarkdown(path, bytes);
		} catch {
			// A committed file must never fail its mutation after the fact.
			return {
				kind: 'malformed',
				title: '',
				body: '',
				error: 'The file could not be read',
			};
		}
		// The file's location is canonical; the parser may not know it.
		return parsed.kind === 'managed'
			? {
					...parsed,
					relativePath: path,
					revision: this.#format.contentRevision(bytes),
				}
			: parsed;
	}

	#remember(path: string, bytes: Uint8Array) {
		const cache = this.#cache;
		if (!cache) return;
		cache.files.set(path, {
			revision: this.#format.contentRevision(bytes),
			parsed: this.#parse(path, bytes),
		});
		this.#addFolders(cache, parentPath(path), true);
		this.#result = null;
	}

	#forget(path: string) {
		if (!this.#cache) return;
		this.#cache.files.delete(path);
		this.#result = null;
	}

	#drop() {
		this.#cache = null;
		this.#result = null;
	}

	#addFolders(cache: WorkspaceCache, folder: string, withAncestors: boolean) {
		let current = folder;
		while (current.length > 0) {
			if (!isExcludedFromSnapshot(current)) cache.folders.add(current);
			if (!withAncestors) return;
			current = parentPath(current);
		}
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

	/** A file cannot replace a folder of the same name. */
	async #assertNotFolder(path: string): Promise<void> {
		if (this.#cache?.folders.has(path))
			throw new BrowserStorageError(
				'path_exists',
				'A folder already exists at the destination',
			);
	}

	#storedFile(path: string, bytes: Uint8Array): StoredFile {
		return { path, bytes, revision: this.#format.contentRevision(bytes) };
	}

	async #stageAndCommit(
		journal: Extract<Journal, { kind: 'write' | 'move' }>,
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
		if (journal.kind === 'moves') {
			// Each step is idempotent: a source that is already gone moved
			// before the interruption.
			for (const move of journal.moves) {
				const bytes = await this.#fileSystem.read(move.from);
				if (bytes === null) continue;
				await this.#fileSystem.write(move.to, bytes);
				await this.#fileSystem.remove(move.from);
			}
			await this.#fileSystem.remove(journalPath(journal.id));
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
		let recovered = false;
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
			recovered = true;
		}
		// A recovered journal changed files behind the projection.
		if (recovered) this.#drop();
	}

	async #enumerate(
		directory: string,
	): Promise<{ files: string[]; folders: string[] }> {
		const entries = await this.#fileSystem.list(directory);
		const files: string[] = [];
		const folders: string[] = [];
		for (const entry of entries.sort((left, right) =>
			compareCodeUnits(left.name, right.name),
		)) {
			const path =
				directory.length === 0 ? entry.name : `${directory}/${entry.name}`;
			if (isExcludedFromSnapshot(path)) continue;
			validateRelativePath(path);
			if (entry.kind === 'file') {
				files.push(path);
			} else {
				folders.push(path);
				const nested = await this.#enumerate(path);
				files.push(...nested.files);
				folders.push(...nested.folders);
			}
		}
		return { files, folders };
	}
}

/** Orders strings by UTF-16 code unit, independent of the user's locale. */
function compareCodeUnits(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
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
			value.kind === 'moves' &&
			Array.isArray(value.moves) &&
			hasOnlyKeys(value, ['version', 'id', 'kind', 'moves'])
		) {
			if (
				value.moves.length === 0 ||
				value.moves.length > BROWSER_SNAPSHOT_LIMITS.maxEntries
			)
				return malformedJournal();
			const moves = value.moves.map((move) => {
				if (
					!isRecord(move) ||
					typeof move.from !== 'string' ||
					typeof move.to !== 'string' ||
					move.from === move.to ||
					!hasOnlyKeys(move, ['from', 'to'])
				)
					return malformedJournal();
				return {
					from: validateRelativePath(move.from),
					to: validateRelativePath(move.to),
				};
			});
			return { version: 1, id, kind: 'moves', moves };
		}
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

/**
 * Durable metadata is retained, but only visible workspace files define live
 * objects. This matches the native scanner: nothing under a dot folder, no dot
 * files, and no top-level `node_modules` or `target` folder.
 */
export function isLiveWorkspaceObjectPath(path: string): boolean {
	const parts = path.split('/');
	if (parts[0] === 'node_modules' || parts[0] === 'target') return false;
	return parts.every((part) => !part.startsWith('.'));
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

	async makeDirectory(path: string): Promise<void> {
		await this.#directory(path, true);
	}

	async removeDirectory(
		path: string,
		options: { recursive?: boolean } = {},
	): Promise<boolean> {
		try {
			const parent = await this.#directory(parentPath(path), false);
			await parent.removeEntry(fileName(path), {
				recursive: options.recursive === true,
			});
			return true;
		} catch (error) {
			if (isNotFound(error)) return false;
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
