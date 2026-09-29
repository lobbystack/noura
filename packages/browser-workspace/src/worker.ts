import {
	BROWSER_EXPORT_SNAPSHOT_LIMITS,
	BROWSER_SNAPSHOT_LIMITS,
	BrowserStorageError,
	BrowserWorkspaceStorage,
	createBrowserStorageLock,
	isExcludedFromSnapshot,
	isLiveWorkspaceObjectPath,
	OpfsFileSystem,
	type BrowserWorkspaceSnapshotEntry,
	type RebuildResult,
	type RebuiltManagedFile,
	validateRelativePath,
} from '@noura/browser-storage';
import type {
	CalendarEntry,
	CoreError,
	CoreEvent,
	EventSource,
	ManagedDraftInput,
	ManagedDraftResult,
	MarkdownLinkTarget,
	MutationResult,
	RawMarkdownRead,
	WorkspaceObject,
	WorkspaceState,
} from '@noura/shared';
import {
	loadWorkspaceFormat,
	type WorkspaceFormat,
	type WorkspaceFormatError,
} from '@noura/workspace-format-wasm';
import type {
	BrowserWorkerPort,
	BrowserWorkerRequest,
	BrowserWorkerResponse,
} from './protocol';
import {
	calendarCandidates,
	diagnostics,
	indexedFileCount,
	isVisiblePath,
	listEntries,
	listFolders,
	listNonManagedMarkdown,
	OBJECT_EVENT_BATCH_LIMIT,
	objectChanges,
	objectHeads,
	queryObjects,
	querySummaries,
	search,
	toObject,
	type ObjectHead,
} from './queries';

const MANIFEST_PATH = '.noura/workspace.yaml';
const COLLECTION_DIRECTORY = '.noura/browser-workspaces';
const TRASH_DIRECTORY = '.noura/trash';
const HISTORY_DIRECTORY = '.noura/history';
/** Browser workspaces are addressed as `browser://<workspace-id>`. */
export const BROWSER_WORKSPACE_PATH_PREFIX = 'browser://';
const BROWSER_PATH_PREFIX = BROWSER_WORKSPACE_PATH_PREFIX;
const SNAPSHOT_FORMAT = 'noura.workspace-snapshot';
/** Native `MAX_ASSET_BYTES`: the largest image a note previews inline. */
const MAX_ASSET_BYTES = 20 * 1024 * 1024;
const MIME_BY_EXTENSION: Record<string, string> = {
	apng: 'image/apng',
	avif: 'image/avif',
	gif: 'image/gif',
	jpeg: 'image/jpeg',
	jpg: 'image/jpeg',
	png: 'image/png',
	svg: 'image/svg+xml',
	webp: 'image/webp',
};
const RESERVED_PROPERTIES = new Set(['id', 'type', 'created', 'updated']);

/**
 * What the browser host can do. Everything that needs the operating system
 * (a file manager, a terminal, other apps, the system trash, an MCP client,
 * app updates, a native menu) is off, so the interface hides it.
 */
export const BROWSER_APP_CAPABILITIES = {
	openTerminal: false,
	revealSelectsFile: false,
	revealInFileManager: false,
	openWithDefaultApp: false,
	systemTrash: false,
	workspaceFolders: false,
	mcp: false,
	appUpdates: false,
	launchAtLogin: false,
	nativeMenu: false,
	ai: false,
} as const;

/**
 * Plugins a brand-new browser workspace enables immediately, so it is usable
 * without a manual trip to settings. Native `create_with_identity` seeds
 * `folders`, `notes`, `tasks`, `calendar` and `projects`; the browser seeds the
 * same set. An explicit `manifest_update` always overrides this seed.
 */
const DEFAULT_ENABLED_PLUGINS = [
	'calendar',
	'folders',
	'notes',
	'projects',
	'tasks',
] as const;

/**
 * A lossless structured-clone artifact. A UI may later wrap these entries in a
 * downloadable archive without changing canonical workspace bytes.
 */
export type BrowserWorkspaceSnapshot = {
	format: typeof SNAPSHOT_FORMAT;
	version: 1;
	workspaceId: string;
	entries: BrowserWorkspaceSnapshotEntry[];
};

/** How workers in other tabs announce changes to the same workspace. */
export interface WorkspaceChangeChannel {
	postMessage(message: WorkspaceChangeMessage): void;
	addEventListener(
		type: 'message',
		listener: (event: MessageEvent<WorkspaceChangeMessage>) => void,
	): void;
	close(): void;
}

export type WorkspaceChangeMessage = {
	type: 'workspace-changed';
	/** The sending worker, so it ignores its own announcements. */
	origin: string;
	/** Changed canonical paths, or null when anything may have changed. */
	paths: string[] | null;
};

type WorkspaceHandle = {
	id: string;
	name: string;
	storage: BrowserWorkspaceStorage;
	channel: WorkspaceChangeChannel | null;
	/** Disposable plugin cache state; closing the workspace discards it. */
	pluginState: Map<string, unknown>;
};

type ManagedType = 'note' | 'task' | 'project';

export interface BrowserWorkspaceRegistry {
	/** Allocates a workspace ID once and never returns an existing directory. */
	createFresh(id: string): Promise<BrowserWorkspaceStorage>;
	open(id: string): Promise<BrowserWorkspaceStorage | null>;
	list(): Promise<Array<{ id: string; storage: BrowserWorkspaceStorage }>>;
}

export type BrowserWorkspaceServerOptions = {
	format: WorkspaceFormat;
	registry: BrowserWorkspaceRegistry;
	now?: () => string;
	onEvent?: (event: CoreEvent) => void;
	/** Opens the channel shared with other tabs for one workspace. */
	openChannel?: (workspaceId: string) => WorkspaceChangeChannel | null;
};

function defaultChannel(workspaceId: string): WorkspaceChangeChannel | null {
	if (typeof BroadcastChannel === 'undefined') return null;
	return new BroadcastChannel(
		`noura:browser-workspace:${workspaceId}`,
	) as unknown as WorkspaceChangeChannel;
}

/**
 * Serves the typed client's commands for workspaces stored in the browser.
 * It answers the same commands as the native engine with the same shapes, so
 * the app renders the same screens on both. Commands that need the operating
 * system fail with `browser_operation_unsupported`.
 */
export class BrowserWorkspaceServer {
	readonly #format: WorkspaceFormat;
	readonly #registry: BrowserWorkspaceRegistry;
	readonly #now: () => string;
	readonly #onEvent: ((event: CoreEvent) => void) | undefined;
	readonly #openChannel: (workspaceId: string) => WorkspaceChangeChannel | null;
	readonly #instance = crypto.randomUUID();
	#current: WorkspaceHandle | null = null;
	#requests: Promise<void> = Promise.resolve();

	constructor({
		format,
		registry,
		now = () => new Date().toISOString(),
		onEvent,
		openChannel = defaultChannel,
	}: BrowserWorkspaceServerOptions) {
		this.#format = format;
		this.#registry = registry;
		this.#now = now;
		this.#onEvent = onEvent;
		this.#openChannel = openChannel;
	}

	request(
		command: string,
		payload: Record<string, unknown> = {},
	): Promise<unknown> {
		return this.#serialize(async () => {
			try {
				return await this.#route(command, payload);
			} catch (error) {
				throw asCoreError(error, command, this.#current?.id);
			}
		});
	}

	#serialize<T>(operation: () => Promise<T>): Promise<T> {
		const result = this.#requests.then(operation, operation);
		this.#requests = result.then(
			() => undefined,
			() => undefined,
		);
		return result;
	}

	async #route(command: string, payload: Record<string, unknown>) {
		switch (command) {
			case 'workspace_create':
				return this.#createWorkspace(payload);
			case 'workspace_open':
				return this.#openWorkspace(payload);
			case 'workspace_close':
				return this.#closeWorkspace();
			case 'workspace_state':
				return this.#state();
			case 'workspace_list_recent':
				return this.#listWorkspaces();
			case 'workspace_export':
				return this.#exportWorkspace();
			case 'workspace_import':
				return this.#importWorkspace(payload);
			case 'workspace_rebuild_index':
				return this.#rebuild();
			case 'manifest_read':
				return this.#manifest();
			case 'manifest_update':
				return this.#updateManifest(payload);
			case 'app_capabilities':
				return { ...BROWSER_APP_CAPABILITIES };
			case 'app_diagnostics':
				return this.#diagnosticsReport();
			case 'objects_query':
				return this.#queryObjects(payload);
			case 'objects_get':
				return this.#getObject(payload);
			case 'objects_summaries':
				return querySummaries(
					await this.#snapshot('objects_summaries'),
					record(payload.query ?? {}, 'objects_summaries'),
				);
			case 'objects_create':
				return this.#createObject(payload);
			case 'objects_update':
				return this.#updateObject(payload);
			case 'objects_move':
				return this.#moveObject(payload);
			case 'objects_delete':
				return this.#deleteObject(payload);
			case 'objects_adopt':
				return this.#adoptObject(payload);
			case 'notes_reconcile_draft':
				return this.#reconcileNoteDraft(payload);
			case 'notes_resolve_conflict':
				return this.#resolveNoteConflict(payload);
			case 'managed_draft_reconcile':
				return this.#reconcileManagedDraft(payload);
			case 'managed_draft_save':
				return this.#saveManagedDraft(payload);
			case 'managed_conflict_resolve':
				return this.#resolveManagedConflict(payload);
			case 'files_list':
				return listEntries(await this.#snapshot('files_list'));
			case 'files_list_non_managed_markdown':
				return listNonManagedMarkdown(
					await this.#snapshot('files_list_non_managed_markdown'),
				);
			case 'files_move':
				return this.#moveFile(payload);
			case 'files_trash':
				return this.#trashPath(payload);
			case 'files_copy':
				return this.#copyFile(payload);
			case 'raw_markdown_read':
				return this.#readRaw(
					string(payload.relativePath, 'relativePath', 'raw_markdown_read'),
					'raw_markdown_read',
				);
			case 'raw_markdown_save':
				return this.#saveRaw(payload);
			case 'raw_markdown_reconcile':
				return this.#reconcileRaw(payload);
			case 'raw_markdown_resolve':
				return this.#resolveRaw(payload);
			case 'files_resolve_markdown_link':
				return this.#resolveLink(payload);
			case 'files_read_local_asset':
				return this.#readAsset(payload);
			case 'files_inspect_pdf':
				return this.#inspectPdf(payload);
			case 'files_read_pdf_range':
				return this.#readPdfRange(payload);
			case 'folders_list':
				return listFolders(await this.#snapshot('folders_list'));
			case 'folders_create':
				return this.#createFolder(payload);
			case 'folders_move':
				return this.#moveFolder(payload);
			case 'folders_remove':
				return this.#removeFolder(payload);
			case 'search_query':
				return this.#search(payload);
			case 'calendar_query':
				return this.#calendar(payload);
			case 'plugin_state_get':
				return this.#pluginState('plugin_state_get', payload).get() ?? null;
			case 'plugin_state_set':
				return this.#pluginState('plugin_state_set', payload).set(
					payload.value,
				);
			case 'plugin_state_delete':
				return this.#pluginState('plugin_state_delete', payload).delete();
			case 'collaboration_open':
				// Browser sync works per file; there are no live sessions.
				return null;
			case 'storage_files_list':
				return this.#listStorageFiles();
			case 'storage_objects_list':
				return this.#listObjectCards();
			case 'storage_files_read':
				return this.#readFile(payload);
			case 'storage_files_write':
				return this.#writeFile(payload);
			case 'storage_files_move':
				return this.#moveStorageFile(payload);
			case 'storage_files_delete':
				return this.#deleteFile(payload);
			default:
				throw coreError(
					'browser_operation_unsupported',
					'validation',
					'This isn’t available in the browser',
					command,
				);
		}
	}

	// --- Workspaces ------------------------------------------------------

	async #createWorkspace(
		payload: Record<string, unknown>,
	): Promise<WorkspaceState> {
		const input = record(payload.input, 'workspace_create');
		if (input.path !== BROWSER_PATH_PREFIX)
			throw coreError(
				'invalid_browser_path',
				'validation',
				'Browser workspaces must use browser:// as their creation path',
				'workspace_create',
			);
		const name = string(input.name, 'name', 'workspace_create');
		const created = this.#now();
		const manifest = this.#format.updateWorkspaceManifest(
			this.#format.createWorkspaceManifest(name, created),
			{ enabledPlugins: [...DEFAULT_ENABLED_PLUGINS] },
			created,
		);
		const storage = await this.#registry.createFresh(manifest.id);
		const bytes = new TextEncoder().encode(
			this.#format.serializeWorkspaceManifest(manifest),
		);
		await storage.write({ path: MANIFEST_PATH, bytes, expectedRevision: null });
		this.#adopt(manifest.id, manifest.name, storage);
		return this.#ready();
	}

	async #openWorkspace(
		payload: Record<string, unknown>,
	): Promise<WorkspaceState> {
		const input = record(payload.input, 'workspace_open');
		const id = browserWorkspaceId(
			string(input.path, 'path', 'workspace_open'),
			this.#format,
		);
		if (this.#current?.id === id) return this.#state();
		const storage = await this.#registry.open(id);
		if (!storage)
			throw coreError(
				'workspace_not_found',
				'validation',
				'The browser workspace does not exist',
				'workspace_open',
			);
		const manifest = await this.#readManifest(storage);
		if (manifest.id !== id)
			throw coreError(
				'workspace_identity_mismatch',
				'validation',
				'The browser workspace manifest does not match its stable workspace ID',
				'workspace_open',
			);
		this.#adopt(manifest.id, manifest.name, storage);
		return this.#ready();
	}

	#adopt(id: string, name: string, storage: BrowserWorkspaceStorage) {
		this.#release();
		const channel = this.#openChannel(id);
		const handle: WorkspaceHandle = {
			id,
			name,
			storage,
			channel,
			pluginState: new Map(),
		};
		channel?.addEventListener('message', (event) => {
			const message = event.data;
			if (
				message?.type !== 'workspace-changed' ||
				message.origin === this.#instance
			)
				return;
			void this.#serialize(() =>
				this.#receiveExternal(handle, message.paths),
			).catch(() => {});
		});
		this.#current = handle;
	}

	#release() {
		this.#current?.channel?.close();
		this.#current = null;
	}

	async #ready(): Promise<WorkspaceState> {
		const state = await this.#state();
		this.#emit('workspace:ready', {}, 'reconciliation');
		return state;
	}

	#closeWorkspace() {
		if (this.#current) this.#emit('workspace:closed');
		this.#release();
		return null;
	}

	async #state(): Promise<WorkspaceState> {
		if (!this.#current)
			return { phase: 'idle', indexedFiles: 0, diagnostics: [] };
		const rebuilt = await this.#current.storage.rebuild();
		return {
			phase: 'ready',
			workspaceId: this.#current.id,
			rootPath: `${BROWSER_PATH_PREFIX}${this.#current.id}`,
			indexedFiles: indexedFileCount(rebuilt),
			diagnostics: diagnostics(rebuilt),
		};
	}

	async #listWorkspaces() {
		const values = await this.#registry.list();
		const result: Array<{ path: string; name: string; workspaceId: string }> =
			[];
		for (const { id, storage } of values) {
			if (!this.#format.isValidObjectId(id, 'workspace')) continue;
			const manifest = await this.#readManifest(storage).catch(() => null);
			if (manifest?.id === id)
				result.push({
					path: `${BROWSER_PATH_PREFIX}${id}`,
					name: manifest.name,
					workspaceId: manifest.id,
				});
		}
		return result.sort((left, right) => left.name.localeCompare(right.name));
	}

	async #exportWorkspace(): Promise<BrowserWorkspaceSnapshot> {
		const current = this.#requireCurrent('workspace_export');
		const manifest = await this.#readManifest(current.storage);
		if (manifest.id !== current.id)
			throw coreError(
				'workspace_identity_mismatch',
				'identity',
				'The browser workspace manifest does not match its stable workspace ID',
				'workspace_export',
			);
		return {
			format: SNAPSHOT_FORMAT,
			version: 1,
			workspaceId: current.id,
			entries: await current.storage.exportSnapshotEntries(
				BROWSER_EXPORT_SNAPSHOT_LIMITS,
			),
		};
	}

	async #importWorkspace(
		payload: Record<string, unknown>,
	): Promise<WorkspaceState> {
		const { snapshot, manifest } = validateSnapshot(payload, this.#format);
		const storage = await this.#registry.createFresh(snapshot.workspaceId);
		await storage.importSnapshotEntries(snapshot.entries);
		this.#adopt(manifest.id, manifest.name, storage);
		return this.#ready();
	}

	async #rebuild(): Promise<WorkspaceState> {
		const current = this.#requireCurrent('workspace_rebuild_index');
		const before = objectHeads(await current.storage.rebuild());
		await current.storage.invalidate();
		await this.#emitDiff(before, 'reconciliation');
		return this.#state();
	}

	async #diagnosticsReport(): Promise<string> {
		const lines = ['noura in the browser'];
		if (typeof navigator !== 'undefined')
			lines.push(`browser: ${navigator.userAgent}`);
		const current = this.#current;
		if (!current) {
			lines.push('workspace: none open');
			return lines.join('\n');
		}
		const rebuilt = await current.storage.rebuild();
		lines.push(
			`files: ${rebuilt.files.length}`,
			`markdown: ${indexedFileCount(rebuilt)}`,
			`objects: ${rebuilt.managed.length}`,
		);
		const counts = new Map<string, number>();
		for (const value of diagnostics(rebuilt))
			counts.set(value.code, (counts.get(value.code) ?? 0) + 1);
		for (const [code, count] of counts) lines.push(`${code}: ${count}`);
		return lines.join('\n');
	}

	// --- Manifest --------------------------------------------------------

	async #manifest() {
		return this.#readManifest(this.#requireCurrent('manifest_read').storage);
	}

	async #updateManifest(payload: Record<string, unknown>) {
		const input = record(payload.input, 'manifest_update');
		const current = this.#requireCurrent('manifest_update');
		const stored = await current.storage.read(MANIFEST_PATH);
		if (!stored)
			throw coreError(
				'workspace_manifest_missing',
				'parse',
				'The browser workspace manifest is missing',
				'manifest_update',
			);
		const manifest = this.#format.parseWorkspaceManifest(stored.bytes);
		const expectedUpdated = optionalNullableString(
			input.expectedUpdated,
			'expectedUpdated',
			'manifest_update',
		);
		if (expectedUpdated !== undefined && manifest.updated !== expectedUpdated)
			throw coreError(
				'manifest_conflict',
				'conflict',
				'.noura/workspace.yaml changed since it was last read',
				'manifest_update',
			);
		const name = optionalNullableString(input.name, 'name', 'manifest_update');
		const enabledPlugins = optionalNullableStrings(
			input.enabledPlugins,
			'enabledPlugins',
			'manifest_update',
		);
		const ignore = optionalNullableStrings(
			input.ignore,
			'ignore',
			'manifest_update',
		);
		if (
			name === undefined &&
			enabledPlugins === undefined &&
			ignore === undefined
		)
			return manifest;
		const updated = this.#format.updateWorkspaceManifest(
			manifest,
			{
				...(name === undefined ? {} : { name }),
				...(enabledPlugins === undefined ? {} : { enabledPlugins }),
				...(ignore === undefined ? {} : { ignore }),
			},
			this.#nextManifestUpdated(manifest.updated),
		);
		const bytes = new TextEncoder().encode(
			this.#format.serializeWorkspaceManifest(updated),
		);
		await current.storage.write({
			path: MANIFEST_PATH,
			bytes,
			expectedRevision: this.#format.contentRevision(stored.bytes),
		});
		current.name = updated.name;
		this.#emit('workspace:manifest-updated', {
			enabledPlugins: updated.enabled_plugins,
			name: updated.name,
		});
		this.#broadcast([MANIFEST_PATH]);
		return updated;
	}

	/**
	 * `manifest_update` uses the stored `updated` field as its optimistic
	 * concurrency token, exactly like the native engine. The native engine gets
	 * away with a plain clock because it writes nanoseconds under an exclusive
	 * lock; `Date` only has millisecond resolution, so two tabs can otherwise
	 * produce identical tokens and let a stale writer pass the conflict check.
	 * Advance the token past the stored manifest whenever the wall clock has not.
	 */
	#nextManifestUpdated(previous: string): string {
		const candidate = this.#now();
		const candidateTime = Date.parse(candidate);
		const previousTime = Date.parse(previous);
		if (
			Number.isFinite(candidateTime) &&
			Number.isFinite(previousTime) &&
			candidateTime <= previousTime
		)
			return new Date(previousTime + 1).toISOString();
		return candidate;
	}

	// --- Objects ---------------------------------------------------------

	async #queryObjects(
		payload: Record<string, unknown>,
	): Promise<WorkspaceObject[]> {
		const query =
			payload.query === undefined ? {} : record(payload.query, 'objects_query');
		return queryObjects(await this.#snapshot('objects_query'), {
			type: optionalNullableString(query.type, 'type', 'objects_query'),
			project: optionalNullableString(
				query.project,
				'project',
				'objects_query',
			),
			status: optionalNullableString(query.status, 'status', 'objects_query'),
			priority: optionalNullableString(
				query.priority,
				'priority',
				'objects_query',
			),
			pathPrefix: optionalNullableString(
				query.pathPrefix,
				'pathPrefix',
				'objects_query',
			),
		});
	}

	async #getObject(payload: Record<string, unknown>): Promise<WorkspaceObject> {
		const id = string(payload.id, 'id', 'objects_get');
		return toObject(await this.#requireObject(id, 'objects_get'));
	}

	async #createObject(payload: Record<string, unknown>) {
		const input = record(payload.input, 'objects_create');
		const type = managedType(input.type, 'objects_create');
		const relativePath = optionalNullableString(
			input.relativePath,
			'relativePath',
			'objects_create',
		);
		const object = this.#construct(type, {
			title: string(input.title, 'title', 'objects_create'),
			body: typeof input.body === 'string' ? input.body : '',
			relativePath,
			properties:
				input.properties === undefined || input.properties === null
					? {}
					: record(input.properties, 'objects_create'),
		});
		const value = await this.#writeNewObject(object, 'objects_create');
		this.#emit('object:created', objectEventPayload(value));
		this.#broadcast([value.relativePath]);
		return mutation(value);
	}

	/** Builds a new object of `type`, validating its destination. */
	#construct(
		type: ManagedType,
		input: {
			title: string;
			body: string;
			relativePath?: string | undefined;
			properties: Record<string, unknown>;
		},
	): WorkspaceObject {
		if (
			input.relativePath !== undefined &&
			!this.#format.isValidManagedObjectPath(input.relativePath)
		)
			throw coreError(
				'invalid_path',
				'validation',
				'The managed object destination is invalid',
				'objects_create',
			);
		const objectInput = {
			title: input.title,
			body: input.body,
			...(input.relativePath === undefined
				? {}
				: { relativePath: input.relativePath }),
			properties: input.properties,
			now: this.#now(),
		};
		return type === 'task'
			? this.#format.createTask(objectInput)
			: type === 'project'
				? this.#format.createProject(objectInput)
				: this.#format.createNote(objectInput);
	}

	async #writeNewObject(
		object: WorkspaceObject,
		operation: string,
	): Promise<WorkspaceObject> {
		const storage = this.#requireCurrent(operation).storage;
		await this.#assertPathFree(object.relativePath, operation);
		const stored = await storage.write({
			path: object.relativePath,
			bytes: this.#format.serializeObject(object),
			expectedRevision: null,
		});
		return this.#managed(object.relativePath, stored.bytes, operation);
	}

	async #updateObject(payload: Record<string, unknown>) {
		const id = string(payload.id, 'id', 'objects_update');
		const patch = record(payload.patch, 'objects_update');
		const current = await this.#requireObject(id, 'objects_update');
		const expectedRevision = string(
			patch.expectedRevision,
			'expectedRevision',
			'objects_update',
		);
		const title = optionalString(patch.title, 'title', 'objects_update');
		const body = optionalString(patch.body, 'body', 'objects_update');
		const properties =
			patch.properties === undefined
				? undefined
				: record(patch.properties, 'objects_update');
		const object = this.#edit(current, {
			...(title === undefined ? {} : { title }),
			...(body === undefined ? {} : { body }),
			...(properties === undefined ? {} : { properties }),
			removeProperties:
				patch.removeProperties === undefined
					? []
					: strings(
							patch.removeProperties,
							'removeProperties',
							'objects_update',
						),
		});
		const value = await this.#commit(
			object,
			expectedRevision,
			'objects_update',
		);
		this.#emit('object:updated', objectEventPayload(value));
		this.#broadcast([value.relativePath]);
		return mutation(value);
	}

	/** Applies an edit through the canonical type rules. */
	#edit(
		current: RebuiltManagedFile,
		input: {
			title?: string;
			body?: string;
			properties?: Record<string, unknown>;
			removeProperties: string[];
		},
	): WorkspaceObject {
		const type = managedType(current.type, 'objects_update');
		const update = { ...input, now: this.#now() };
		const object = toObject(current);
		return type === 'task'
			? this.#format.updateTask(object, update)
			: type === 'project'
				? this.#format.updateProject(object, update)
				: this.#format.updateNote(object, update);
	}

	/** Replaces an object's file when it still has `expectedRevision`. */
	async #commit(
		object: WorkspaceObject,
		expectedRevision: string,
		operation: string,
	): Promise<WorkspaceObject> {
		const stored = await this.#requireCurrent(operation).storage.write({
			path: object.relativePath,
			bytes: this.#format.serializeObject(object),
			expectedRevision,
		});
		return this.#managed(object.relativePath, stored.bytes, operation);
	}

	async #moveObject(payload: Record<string, unknown>) {
		const input = record(payload.input, 'objects_move');
		const id = string(input.id, 'id', 'objects_move');
		const relativePath = string(
			input.relativePath,
			'relativePath',
			'objects_move',
		);
		const expectedRevision = string(
			input.expectedRevision,
			'expectedRevision',
			'objects_move',
		);
		if (!this.#format.isValidManagedObjectPath(relativePath))
			throw coreError(
				'invalid_path',
				'validation',
				'The managed object destination is invalid',
				'objects_move',
			);
		const current = await this.#requireObject(id, 'objects_move');
		const stored = await this.#requireCurrent('objects_move').storage.move({
			from: current.relativePath,
			to: relativePath,
			expectedRevision,
			expectedDestinationRevision: null,
		});
		const value = this.#managed(relativePath, stored.bytes, 'objects_move');
		this.#emit('object:moved', {
			id: value.id,
			type: value.type,
			from: current.relativePath,
			to: relativePath,
			previousPath: current.relativePath,
			path: relativePath,
			revision: value.revision,
		});
		this.#broadcast([current.relativePath, relativePath]);
		return mutation(value);
	}

	async #deleteObject(payload: Record<string, unknown>) {
		const input = record(payload.input, 'objects_delete');
		const id = string(input.id, 'id', 'objects_delete');
		const expectedRevision = string(
			input.expectedRevision,
			'expectedRevision',
			'objects_delete',
		);
		const current = await this.#requireObject(id, 'objects_delete');
		const trashPath = this.#trashPathFor(current.relativePath);
		await this.#requireCurrent('objects_delete').storage.move({
			from: current.relativePath,
			to: trashPath,
			expectedRevision,
			expectedDestinationRevision: null,
		});
		this.#emit('object:deleted', {
			id: current.id,
			type: current.type,
			path: current.relativePath,
			trashPath,
			revision: current.revision,
		});
		this.#broadcast([current.relativePath, trashPath]);
		return mutation(toObject(current));
	}

	async #adoptObject(payload: Record<string, unknown>) {
		const input = record(payload.input, 'objects_adopt');
		const type = managedType(input.type, 'objects_adopt');
		const relativePath = validPath(
			string(input.relativePath, 'relativePath', 'objects_adopt'),
			'objects_adopt',
		);
		const expectedRevision = string(
			input.expectedRevision,
			'expectedRevision',
			'objects_adopt',
		);
		const storage = this.#requireCurrent('objects_adopt').storage;
		const stored = await storage.read(relativePath);
		if (!stored) throw fileNotFound('objects_adopt');
		if (stored.revision !== expectedRevision)
			throw revisionConflict('objects_adopt');
		const parsed = this.#format.parseMarkdown(relativePath, stored.bytes);
		if (parsed.kind !== 'unmanaged')
			throw coreError(
				'not_adoptable',
				'validation',
				'Only idless Markdown can be adopted',
				'object_adopt',
			);
		const properties = { ...(parsed.frontmatter ?? {}) };
		for (const key of RESERVED_PROPERTIES) delete properties[key];
		const created = this.#construct(type, {
			title: parsed.title.length > 0 ? parsed.title : fileStem(relativePath),
			body: parsed.body,
			relativePath,
			properties,
		});
		const frontmatter = parsed.frontmatter ?? {};
		const object = {
			...created,
			created:
				typeof frontmatter.created === 'string'
					? frontmatter.created
					: created.created,
			updated:
				typeof frontmatter.updated === 'string'
					? frontmatter.updated
					: created.updated,
		};
		const value = await this.#commit(object, expectedRevision, 'objects_adopt');
		this.#emit('object:created', objectEventPayload(value));
		this.#broadcast([relativePath]);
		return mutation(value);
	}

	// --- Drafts ----------------------------------------------------------

	async #reconcileNoteDraft(payload: Record<string, unknown>) {
		const input = record(payload.input, 'note_reconcile');
		const id = string(input.id, 'id', 'note_reconcile');
		const current = await this.#requireObject(id, 'note_reconcile');
		if (current.type !== 'note') throw typeMismatch('note_reconcile');
		const localBody = string(input.localBody, 'localBody', 'note_reconcile');
		if (current.revision === input.baseRevision)
			return {
				status: 'unchanged',
				current: toObject(current),
				body: localBody,
			};
		const body = this.#format.mergeMarkdownBody(
			string(input.baseBody, 'baseBody', 'note_reconcile'),
			localBody,
			current.body,
		);
		if (body === null)
			return { status: 'conflict', current: toObject(current) };
		await this.#snapshotObject(current, 'external');
		return { status: 'merged', current: toObject(current), body };
	}

	async #resolveNoteConflict(payload: Record<string, unknown>) {
		const operation = 'note_conflict_resolve';
		const input = record(payload.input, operation);
		const current = await this.#requireObject(
			string(input.id, 'id', operation),
			operation,
		);
		if (current.type !== 'note') throw typeMismatch(operation);
		const currentRevision = string(
			input.currentRevision,
			'currentRevision',
			operation,
		);
		if (current.revision !== currentRevision)
			throw changedDuringReview(operation, current.revision);
		const localBody = string(input.localBody, 'localBody', operation);
		if (input.resolution === 'use-external') {
			await this.#snapshotBytes(
				current.id,
				'local',
				this.#format.serializeObject({ ...toObject(current), body: localBody }),
			);
			return mutation(toObject(current));
		}
		await this.#snapshotObject(current, 'external');
		const value = await this.#commit(
			{ ...toObject(current), body: localBody, updated: this.#now() },
			currentRevision,
			operation,
		);
		this.#emit('object:updated', objectEventPayload(value));
		this.#broadcast([value.relativePath]);
		return mutation(value);
	}

	#draftInput(payload: Record<string, unknown>, operation: string) {
		const input = record(payload.input, operation);
		return {
			id: string(input.id, 'id', operation),
			baseRevision: string(input.baseRevision, 'baseRevision', operation),
			baseTitle: string(input.baseTitle, 'baseTitle', operation),
			baseBody: string(input.baseBody, 'baseBody', operation),
			baseProperties: record(input.baseProperties, operation),
			localTitle: string(input.localTitle, 'localTitle', operation),
			localBody: string(input.localBody, 'localBody', operation),
			localProperties: record(input.localProperties, operation),
		} satisfies ManagedDraftInput;
	}

	#unchanged(input: ManagedDraftInput, canonical: RebuiltManagedFile) {
		return (
			canonical.revision === input.baseRevision &&
			canonical.title === input.baseTitle &&
			canonical.body === input.baseBody &&
			sameJson(
				normalizedProperties(canonical.properties),
				normalizedProperties(input.baseProperties),
			)
		);
	}

	async #reconcileManagedDraft(
		payload: Record<string, unknown>,
	): Promise<ManagedDraftResult> {
		const operation = 'managed_draft_reconcile';
		const input = this.#draftInput(payload, operation);
		const canonical = await this.#requireObject(input.id, operation);
		if (this.#unchanged(input, canonical))
			return { status: 'unchanged', current: toObject(canonical) };
		const merged = this.#format.mergeManagedDraft(input, toObject(canonical));
		if (merged === null)
			return { status: 'conflict', current: toObject(canonical) };
		return {
			status: 'merged',
			current: toObject(canonical),
			title: merged.title,
			body: merged.body,
			properties: merged.properties,
		};
	}

	async #saveManagedDraft(
		payload: Record<string, unknown>,
	): Promise<ManagedDraftResult> {
		const operation = 'managed_draft_save';
		const input = this.#draftInput(payload, operation);
		const canonical = await this.#requireObject(input.id, operation);
		if (this.#unchanged(input, canonical)) {
			const value = await this.#replace(
				canonical,
				{
					title: input.localTitle,
					body: input.localBody,
					properties: input.localProperties,
				},
				canonical.revision,
				operation,
			);
			return { status: 'unchanged', current: value };
		}
		const merged = this.#format.mergeManagedDraft(input, toObject(canonical));
		if (merged === null)
			return { status: 'conflict', current: toObject(canonical) };
		await this.#snapshotObject(canonical, 'external');
		const value = await this.#replace(
			canonical,
			merged,
			canonical.revision,
			operation,
		);
		return {
			status: 'merged',
			current: value,
			title: value.title,
			body: value.body,
			properties: value.properties,
		};
	}

	async #resolveManagedConflict(
		payload: Record<string, unknown>,
	): Promise<WorkspaceObject> {
		const operation = 'managed_conflict_resolve';
		const input = record(payload.input, operation);
		const id = string(input.id, 'id', operation);
		const local = {
			title: string(input.localTitle, 'localTitle', operation),
			body: string(input.localBody, 'localBody', operation),
			properties: record(input.localProperties, operation),
		};
		const replace = input.resolution === 'replace-external';
		const rebuilt = await this.#snapshot(operation);
		const current = this.#findObject(rebuilt, id, operation);
		if (!current) {
			if (!replace) throw objectNotFound(operation);
			return this.#restoreDeleted(id, input, local, operation);
		}
		const currentRevision = string(
			input.currentRevision,
			'currentRevision',
			operation,
		);
		if (current.revision !== currentRevision)
			throw changedDuringReview(operation, current.revision);
		if (!replace) {
			await this.#snapshotBytes(
				current.id,
				'local',
				this.#format.serializeObject({
					...toObject(current),
					title: local.title,
					body: local.body,
					properties: normalizedProperties(local.properties),
				}),
			);
			return toObject(current);
		}
		await this.#snapshotObject(current, 'external');
		return this.#replace(current, local, currentRevision, operation);
	}

	async #restoreDeleted(
		id: string,
		input: Record<string, unknown>,
		local: { title: string; body: string; properties: Record<string, unknown> },
		operation: string,
	): Promise<WorkspaceObject> {
		const relativePath = validPath(
			string(input.relativePath, 'relativePath', operation),
			operation,
		);
		const type = id.split('_')[0] ?? '';
		if (!supportedType(type) || !this.#format.isValidObjectId(id, type))
			throw coreError(
				'invalid_object_id',
				'validation',
				'The object ID is invalid',
				operation,
			);
		const now = this.#now();
		const created = optionalNullableString(input.created, 'created', operation);
		const shell = this.#construct(type, {
			title: local.title,
			body: local.body,
			relativePath,
			properties: normalizedProperties(local.properties),
		});
		const object: WorkspaceObject = {
			...shell,
			id,
			created: created ?? now,
			updated: now,
		};
		const value = await this.#writeNewObject(object, operation);
		this.#emit('object:updated', objectEventPayload(value));
		this.#broadcast([value.relativePath]);
		return value;
	}

	/** Native `apply_managed_object`: replace title, body and properties. */
	async #replace(
		current: RebuiltManagedFile,
		next: { title: string; body: string; properties: Record<string, unknown> },
		expectedRevision: string,
		operation: string,
	): Promise<WorkspaceObject> {
		const properties = normalizedProperties(next.properties);
		const object = this.#edit(current, {
			title: next.title,
			body: next.body,
			properties,
			removeProperties: Object.keys(current.properties).filter(
				(key) => !(key in properties),
			),
		});
		const value = await this.#commit(object, expectedRevision, operation);
		this.#emit('object:updated', objectEventPayload(value));
		this.#broadcast([value.relativePath]);
		return value;
	}

	async #snapshotObject(
		object: RebuiltManagedFile,
		kind: 'local' | 'external',
	) {
		const stored = await this.#requireCurrent('history_snapshot').storage.read(
			object.relativePath,
		);
		if (stored) await this.#snapshotBytes(object.id, kind, stored.bytes);
	}

	/** Keep a displaced version under `.noura/history`, like native. */
	async #snapshotBytes(
		segment: string,
		kind: 'local' | 'external',
		bytes: Uint8Array,
	) {
		const storage = this.#requireCurrent('history_snapshot').storage;
		const path = `${HISTORY_DIRECTORY}/${segment}/${this.#format.contentRevision(bytes)}-${kind}.md`;
		if (await storage.read(path)) return;
		await storage.write({ path, bytes, expectedRevision: null });
	}

	// --- Files and folders ----------------------------------------------

	async #moveFile(payload: Record<string, unknown>) {
		const input = record(payload.input, 'file_move');
		const from = validPath(
			string(input.from, 'from', 'file_move'),
			'file_move',
		);
		const to = validPath(string(input.to, 'to', 'file_move'), 'file_move');
		const rebuilt = await this.#snapshot('file_move');
		if (!rebuilt.files.some((file) => file.path === from))
			throw coreError(
				'file_not_found',
				'validation',
				'The source is not a file in this workspace',
				'file_move',
			);
		await this.#movePaths(from, to, 'file_move');
		return null;
	}

	async #moveFolder(payload: Record<string, unknown>) {
		const input = record(payload.input, 'folder_move');
		const from = validPath(
			string(input.from, 'from', 'folder_move'),
			'folder_move',
		);
		const to = validPath(string(input.to, 'to', 'folder_move'), 'folder_move');
		const rebuilt = await this.#snapshot('folder_move');
		if (!rebuilt.folders.includes(from))
			throw coreError(
				'folder_not_found',
				'validation',
				'The source folder does not exist',
				'folder_move',
			);
		await this.#movePaths(from, to, 'folder_move');
		return null;
	}

	async #trashPath(payload: Record<string, unknown>) {
		const input = record(payload.input, 'file_trash');
		const path = validPath(
			string(input.relativePath, 'relativePath', 'file_trash'),
			'file_trash',
		);
		const trashPath = this.#trashPathFor(path);
		await this.#movePaths(path, trashPath, 'file_trash');
		return trashPath;
	}

	async #movePaths(from: string, to: string, operation: string) {
		const current = this.#requireCurrent(operation);
		const before = objectHeads(await current.storage.rebuild());
		const moves = await current.storage.movePath({ from, to });
		const paths = [from, to, ...moves.flatMap((move) => [move.from, move.to])];
		await this.#emitDiff(before, 'reconciliation');
		this.#emit('file:changed', { paths: [...new Set(paths)] });
		this.#broadcast(paths);
	}

	#trashPathFor(path: string): string {
		const timestamp = this.#now().replaceAll(':', '-').replaceAll('.', '-');
		return `${TRASH_DIRECTORY}/${timestamp}/${path}`;
	}

	async #copyFile(payload: Record<string, unknown>) {
		const operation = 'file_copy';
		const input = record(payload.input, operation);
		const from = validPath(string(input.from, 'from', operation), operation);
		const to = validPath(string(input.to, 'to', operation), operation);
		const current = this.#requireCurrent(operation);
		const source = await current.storage.read(from);
		if (!source)
			throw coreError(
				'file_not_found',
				'validation',
				'The source is not a file in this workspace',
				operation,
			);
		await this.#assertPathFree(to, operation);
		const before = objectHeads(await current.storage.rebuild());
		const parsed =
			isLiveWorkspaceObjectPath(from) && from.endsWith('.md')
				? this.#format.parseMarkdown(from, source.bytes)
				: null;
		if (parsed?.kind === 'managed' && supportedType(parsed.type)) {
			const title = fileStem(to);
			const object = this.#construct(parsed.type, {
				title: title.trim().length === 0 ? parsed.title : title,
				body: parsed.body,
				relativePath: to,
				properties: parsed.properties,
			});
			await this.#writeNewObject(object, operation);
		} else {
			await current.storage.write({
				path: to,
				bytes: source.bytes,
				expectedRevision: null,
			});
		}
		await this.#emitDiff(before, 'reconciliation');
		this.#emit('file:changed', { paths: [to] });
		this.#broadcast([to]);
		return null;
	}

	async #createFolder(payload: Record<string, unknown>) {
		const input = record(payload.input, 'folder_create');
		const path = validPath(
			string(input.relativePath, 'relativePath', 'folder_create'),
			'folder_create',
		);
		await this.#requireCurrent('folder_create').storage.createFolder(path);
		this.#emit('file:changed', { paths: [path] });
		this.#broadcast([path]);
		return null;
	}

	async #removeFolder(payload: Record<string, unknown>) {
		const input = record(payload.input, 'folder_remove');
		const path = validPath(
			string(input.relativePath, 'relativePath', 'folder_remove'),
			'folder_remove',
		);
		await this.#requireCurrent('folder_remove').storage.removeEmptyFolder(path);
		this.#emit('file:changed', { paths: [path] });
		this.#broadcast(null);
		return null;
	}

	// --- Raw Markdown ------------------------------------------------------

	async #readRawBytes(relativePath: string, operation: string) {
		if (!relativePath.toLowerCase().endsWith('.md'))
			throw coreError(
				'invalid_raw_markdown_path',
				'validation',
				'Raw edits are limited to Markdown files',
				'raw_markdown',
			);
		const path = validPath(relativePath, operation);
		const stored = await this.#requireCurrent(operation).storage.read(path);
		if (!stored)
			throw coreError(
				'raw_markdown_missing',
				'validation',
				'The Markdown file no longer exists',
				operation,
			);
		return { path, stored, text: this.#format.readRawText(stored.bytes) };
	}

	async #readRaw(
		relativePath: string,
		operation: string,
	): Promise<RawMarkdownRead> {
		const { path, stored, text } = await this.#readRawBytes(
			relativePath,
			operation,
		);
		return {
			relativePath: path,
			body: text.body,
			revision: stored.revision,
			usesCrlf: text.usesCrlf,
			hasBom: text.hasBom,
		};
	}

	async #reconcileRaw(payload: Record<string, unknown>) {
		const operation = 'raw_markdown_reconcile';
		const input = record(payload.input, operation);
		const current = await this.#readRaw(
			string(input.relativePath, 'relativePath', operation),
			operation,
		);
		if (current.revision === input.baseRevision)
			return { status: 'unchanged', current };
		const merged = this.#format.mergeText(
			string(input.baseBody, 'baseBody', operation),
			string(input.localBody, 'localBody', operation),
			current.body,
		);
		return merged === null
			? { status: 'conflict', current }
			: { status: 'merged', current: { ...current, body: merged } };
	}

	async #saveRaw(payload: Record<string, unknown>) {
		const operation = 'raw_markdown_save';
		const input = record(payload.input, operation);
		const { path, stored, text } = await this.#readRawBytes(
			string(input.relativePath, 'relativePath', operation),
			operation,
		);
		const current: RawMarkdownRead = {
			relativePath: path,
			body: text.body,
			revision: stored.revision,
			usesCrlf: text.usesCrlf,
			hasBom: text.hasBom,
		};
		const localBody = string(input.localBody, 'localBody', operation);
		let body = localBody;
		if (stored.revision !== input.baseRevision) {
			const merged = this.#format.mergeText(
				string(input.baseBody, 'baseBody', operation),
				localBody,
				text.body,
			);
			if (merged === null)
				return { status: 'conflict', current, managedObject: null };
			await this.#snapshotBytes(
				this.#format.rawHistorySegment(path),
				'external',
				stored.bytes,
			);
			body = merged;
		}
		return this.#writeRaw(path, stored, body, operation, 'saved');
	}

	async #writeRaw(
		path: string,
		stored: { bytes: Uint8Array; revision: string },
		body: string,
		operation: string,
		status: 'saved' | null,
	) {
		const next = this.#format.composeRawText(stored.bytes, body);
		const written = await this.#requireCurrent(operation).storage.write({
			path,
			bytes: next,
			expectedRevision: stored.revision,
		});
		this.#emit('file:changed', { paths: [path] });
		this.#emit('search:index-updated');
		this.#broadcast([path]);
		const layout = this.#format.readRawText(next);
		const parsed = this.#format.parseMarkdown(path, next);
		const current: RawMarkdownRead = {
			relativePath: path,
			body,
			revision: written.revision,
			usesCrlf: layout.usesCrlf,
			hasBom: layout.hasBom,
		};
		const managedObject =
			parsed.kind === 'managed'
				? {
						...toObject(parsed),
						relativePath: path,
						revision: written.revision,
					}
				: null;
		return status === null
			? { current, managedObject }
			: { status, current, managedObject };
	}

	async #resolveRaw(payload: Record<string, unknown>) {
		const operation = 'raw_markdown_resolve';
		const input = record(payload.input, operation);
		const { path, stored, text } = await this.#readRawBytes(
			string(input.relativePath, 'relativePath', operation),
			operation,
		);
		if (stored.revision !== input.currentRevision)
			throw changedDuringReview(operation, stored.revision);
		const localBody = string(input.localBody, 'localBody', operation);
		const segment = this.#format.rawHistorySegment(path);
		if (input.resolution === 'use-external') {
			await this.#snapshotBytes(
				segment,
				'local',
				this.#format.composeRawText(stored.bytes, localBody),
			);
			const parsed = this.#format.parseMarkdown(path, stored.bytes);
			return {
				current: {
					relativePath: path,
					body: text.body,
					revision: stored.revision,
					usesCrlf: text.usesCrlf,
					hasBom: text.hasBom,
				},
				managedObject:
					parsed.kind === 'managed'
						? {
								...toObject(parsed),
								relativePath: path,
								revision: stored.revision,
							}
						: null,
			};
		}
		await this.#snapshotBytes(segment, 'external', stored.bytes);
		return this.#writeRaw(path, stored, localBody, operation, null);
	}

	// --- Links, assets and PDFs -------------------------------------------

	async #resolveLink(
		payload: Record<string, unknown>,
	): Promise<MarkdownLinkTarget> {
		const operation = 'markdown_link_resolve';
		const input = record(payload.input, operation);
		const source = string(
			input.sourceRelativePath,
			'sourceRelativePath',
			operation,
		);
		const target = string(input.target, 'target', operation);
		if (target.startsWith('http://') || target.startsWith('https://'))
			return { kind: 'unresolved' };
		if (markdownTargetPath(target, true).length === 0)
			return { kind: 'unresolved' };
		const storage = this.#requireCurrent(operation).storage;
		let relative = resolveMarkdownTarget(source, target, true);
		let stored = await readIfValid(storage, relative);
		if (!stored && !/\.[^/]*$/u.test(baseNameOf(relative))) {
			relative = `${relative}.md`;
			stored = await readIfValid(storage, relative);
		}
		if (!stored) return { kind: 'unresolved' };
		const lower = relative.toLowerCase();
		if (lower.endsWith('.md')) {
			const parsed = this.#format.parseMarkdown(relative, stored.bytes);
			if (parsed.kind === 'managed')
				return {
					kind: 'managed',
					object: {
						...toObject(parsed),
						relativePath: relative,
						revision: stored.revision,
					},
				};
			return {
				kind: 'markdown',
				document: await this.#readRaw(relative, operation),
			};
		}
		if (lower.endsWith('.pdf')) {
			const fragment = target.split('|')[0]!.split('#')[1];
			const page = fragment?.startsWith('page=')
				? Number.parseInt(fragment.slice(5), 10)
				: Number.NaN;
			return {
				kind: 'pdf',
				relativePath: relative,
				page: Number.isInteger(page) && page > 0 ? page : null,
			};
		}
		return { kind: 'asset', relativePath: relative };
	}

	async #readAsset(payload: Record<string, unknown>) {
		const operation = 'raw_asset_read';
		const input = record(payload.input, operation);
		const relative = resolveMarkdownTarget(
			string(input.sourceRelativePath, 'sourceRelativePath', operation),
			string(input.target, 'target', operation),
			false,
		);
		if (relative.toLowerCase().endsWith('.md'))
			throw coreError(
				'invalid_asset_path',
				'validation',
				'Markdown content is read through raw Markdown operations',
				operation,
			);
		const stored = await this.#requireCurrent(operation).storage.read(
			validPath(relative, operation),
		);
		if (!stored) throw fileNotFound(operation);
		if (stored.bytes.byteLength > MAX_ASSET_BYTES)
			throw coreError(
				'asset_too_large',
				'validation',
				'The local asset exceeds the preview size limit',
				operation,
			);
		const extension = relative.split('.').pop()?.toLowerCase() ?? '';
		const mime = MIME_BY_EXTENSION[extension] ?? 'application/octet-stream';
		return {
			dataUrl: `data:${mime};base64,${encodeBase64Bytes(stored.bytes)}`,
		};
	}

	async #inspectPdf(payload: Record<string, unknown>) {
		const operation = 'files_inspect_pdf';
		const relativePath = validPath(
			string(payload.relativePath, 'relativePath', operation),
			operation,
		);
		const current = this.#requireCurrent(operation);
		const stored = await current.storage.read(relativePath);
		if (!stored) throw fileNotFound(operation);
		return {
			relativePath,
			workspaceId: current.id,
			version: stored.revision,
			length: stored.bytes.byteLength,
		};
	}

	async #readPdfRange(payload: Record<string, unknown>) {
		const operation = 'files_read_pdf_range';
		const input = record(payload.input, operation);
		const current = this.#requireCurrent(operation);
		const relativePath = validPath(
			string(input.relativePath, 'relativePath', operation),
			operation,
		);
		const offset = input.offset;
		const length = input.length;
		if (
			input.workspaceId !== current.id ||
			typeof offset !== 'number' ||
			typeof length !== 'number' ||
			!Number.isSafeInteger(offset) ||
			!Number.isSafeInteger(length) ||
			offset < 0 ||
			length < 0 ||
			length > 16 * 1024 * 1024
		)
			throw coreError(
				'invalid_pdf_range',
				'validation',
				'The PDF range is invalid',
				operation,
			);
		const stored = await current.storage.read(relativePath);
		if (!stored || stored.revision !== input.version)
			throw coreError(
				'pdf_changed',
				'conflict',
				'The PDF changed. Open it again.',
				operation,
			);
		return stored.bytes.slice(offset, offset + length);
	}

	// --- Search, calendar and plugin state --------------------------------

	async #search(payload: Record<string, unknown>) {
		const input = record(payload.input, 'search_query');
		return search(await this.#snapshot('search_query'), {
			query: string(input.query, 'query', 'search_query'),
			type: optionalNullableString(input.type, 'type', 'search_query') ?? null,
			pathPrefix:
				optionalNullableString(
					input.pathPrefix,
					'pathPrefix',
					'search_query',
				) ?? null,
			limit: typeof input.limit === 'number' ? input.limit : null,
		});
	}

	async #calendar(payload: Record<string, unknown>): Promise<CalendarEntry[]> {
		const input = record(payload.input, 'calendar_query');
		const start = string(input.start, 'start', 'calendar_query');
		const end = string(input.end, 'end', 'calendar_query');
		return this.#format.selectCalendarEntries(
			calendarCandidates(await this.#snapshot('calendar_query')),
			start,
			end,
		) as CalendarEntry[];
	}

	#pluginState(operation: string, payload: Record<string, unknown>) {
		const state = this.#requireCurrent(operation).pluginState;
		const key = JSON.stringify([
			string(payload.pluginId, 'pluginId', operation),
			string(payload.key, 'key', operation),
		]);
		return {
			get: () => (state.has(key) ? structuredClone(state.get(key)) : undefined),
			set: (value: unknown) => {
				state.set(key, structuredClone(value));
				return null;
			},
			delete: () => state.delete(key),
		};
	}

	// --- Canonical bytes for sync ---------------------------------------

	/**
	 * Raw canonical filesystem operations. These are transport-level primitives
	 * over `BrowserWorkspaceStorage`, which owns path validation, expected
	 * revisions, journaling, and recovery. They perform no workspace-format
	 * interpretation and are the boundary a sync adapter uses to read and apply
	 * canonical bytes.
	 */
	async #listStorageFiles(): Promise<string[]> {
		const rebuilt =
			await this.#requireCurrent('storage_files_list').storage.rebuild();
		return rebuilt.files.map((file) => file.path);
	}

	/**
	 * Minimal managed-object projection for sync bindings. Unlike
	 * `objects_query` it returns no bodies or properties, so a binding can own an
	 * object without reading workspace content through the worker.
	 */
	async #listObjectCards(): Promise<
		Array<{ id: string; path: string; type: string }>
	> {
		const rebuilt = await this.#requireCurrent(
			'storage_objects_list',
		).storage.rebuild();
		return rebuilt.managed
			.filter((value) => supportedType(value.type))
			.map((value) => ({
				id: value.id,
				path: value.relativePath,
				type: value.type,
			}))
			.sort((left, right) => left.path.localeCompare(right.path));
	}

	async #readFile(payload: Record<string, unknown>) {
		const operation = 'storage_files_read';
		if (!hasOnlyKeys(payload, ['path']))
			throw coreError(
				'invalid_input',
				'validation',
				'files_read accepts only a path',
				operation,
			);
		const path = string(payload.path, 'path', operation);
		const stored = await this.#requireCurrent(operation).storage.read(path);
		if (!stored) return null;
		return {
			revision: stored.revision,
			bytes: encodeBase64Bytes(stored.bytes),
		};
	}

	async #writeFile(payload: Record<string, unknown>) {
		const operation = 'storage_files_write';
		if (!hasOnlyKeys(payload, ['path', 'bytes', 'expectedRevision']))
			throw coreError(
				'invalid_input',
				'validation',
				'files_write accepts only path, bytes, and expectedRevision',
				operation,
			);
		const path = string(payload.path, 'path', operation);
		const bytes = decodeBase64Bytes(payload.bytes, 'bytes', operation);
		const expectedRevision = nullableRevision(
			payload.expectedRevision,
			'expectedRevision',
			operation,
		);
		const current = this.#requireCurrent(operation);
		const before = objectHeads(await current.storage.rebuild());
		const stored = await current.storage.write({
			path,
			bytes,
			expectedRevision,
		});
		await this.#afterSyncChange(before, [path]);
		return { path, revision: stored.revision };
	}

	async #moveStorageFile(payload: Record<string, unknown>) {
		const operation = 'storage_files_move';
		if (
			!hasOnlyKeys(payload, [
				'from',
				'to',
				'expectedRevision',
				'expectedDestinationRevision',
			])
		)
			throw coreError(
				'invalid_input',
				'validation',
				'files_move accepts only from, to, expectedRevision, and expectedDestinationRevision',
				operation,
			);
		const from = string(payload.from, 'from', operation);
		const to = string(payload.to, 'to', operation);
		const expectedRevision = string(
			payload.expectedRevision,
			'expectedRevision',
			operation,
		);
		const expectedDestinationRevision = nullableRevision(
			payload.expectedDestinationRevision,
			'expectedDestinationRevision',
			operation,
		);
		const current = this.#requireCurrent(operation);
		const before = objectHeads(await current.storage.rebuild());
		const stored = await current.storage.move({
			from,
			to,
			expectedRevision,
			expectedDestinationRevision,
		});
		await this.#afterSyncChange(before, [from, to]);
		return { path: to, revision: stored.revision };
	}

	async #deleteFile(payload: Record<string, unknown>) {
		const operation = 'storage_files_delete';
		if (!hasOnlyKeys(payload, ['path', 'expectedRevision']))
			throw coreError(
				'invalid_input',
				'validation',
				'files_delete accepts only path and expectedRevision',
				operation,
			);
		const path = string(payload.path, 'path', operation);
		const expectedRevision = string(
			payload.expectedRevision,
			'expectedRevision',
			operation,
		);
		const current = this.#requireCurrent(operation);
		const before = objectHeads(await current.storage.rebuild());
		await current.storage.delete({ path, expectedRevision });
		await this.#afterSyncChange(before, [path]);
		return null;
	}

	/** Sync applied remote bytes: tell this tab and the others. */
	async #afterSyncChange(before: Map<string, ObjectHead>, paths: string[]) {
		await this.#emitDiff(before, 'external');
		this.#emit('file:changed', { paths }, 'external');
		if (paths.includes(MANIFEST_PATH)) await this.#manifestChanged('external');
		this.#broadcast(paths);
	}

	// --- Shared helpers ---------------------------------------------------

	async #snapshot(operation: string): Promise<RebuildResult> {
		return this.#requireCurrent(operation).storage.rebuild();
	}

	#findObject(
		rebuilt: RebuildResult,
		id: string,
		operation: string,
	): RebuiltManagedFile | null {
		if (rebuilt.duplicates.some((value) => value.id === id))
			throw coreError(
				'identity_conflict',
				'identity',
				'More than one file uses this stable ID',
				operation,
			);
		return rebuilt.managed.find((value) => value.id === id) ?? null;
	}

	async #requireObject(
		id: string,
		operation: string,
	): Promise<RebuiltManagedFile> {
		const object = this.#findObject(
			await this.#snapshot(operation),
			id,
			operation,
		);
		if (!object) throw objectNotFound(operation);
		return object;
	}

	async #assertPathFree(path: string, operation: string) {
		const rebuilt = await this.#snapshot(operation);
		if (
			rebuilt.files.some((file) => file.path === path) ||
			rebuilt.folders.includes(path)
		)
			throw coreError(
				'path_exists',
				'conflict',
				'A file already exists at the destination',
				operation,
			);
	}

	#managed(
		path: string,
		bytes: Uint8Array,
		operation: string,
	): WorkspaceObject {
		const parsed = this.#format.parseMarkdown(path, bytes);
		if (parsed.kind !== 'managed')
			throw coreError(
				'canonical_serialization_failed',
				'parse',
				'Canonical object bytes could not be read back',
				operation,
			);
		return {
			...toObject(parsed),
			relativePath: path,
			revision: this.#format.contentRevision(bytes),
		};
	}

	async #readManifest(storage: BrowserWorkspaceStorage) {
		const stored = await storage.read(MANIFEST_PATH);
		if (!stored)
			throw coreError(
				'workspace_manifest_missing',
				'parse',
				'The browser workspace manifest is missing',
				'manifest_read',
			);
		return this.#format.parseWorkspaceManifest(stored.bytes);
	}

	#requireCurrent(operation: string): WorkspaceHandle {
		if (!this.#current)
			throw coreError(
				'workspace_not_open',
				'validation',
				'Open a browser workspace first',
				operation,
			);
		return this.#current;
	}

	// --- Events -----------------------------------------------------------

	#emit(
		type: string,
		payload: unknown = {},
		source: EventSource = 'application',
	) {
		if (!this.#current) return;
		this.#onEvent?.({
			eventId: crypto.randomUUID(),
			type,
			workspaceId: this.#current.id,
			occurredAt: this.#now(),
			source,
			payload,
		});
	}

	/** Announce object changes since `before`, batched like native. */
	async #emitDiff(before: Map<string, ObjectHead>, source: EventSource) {
		const current = this.#current;
		if (!current) return;
		const changes = objectChanges(
			before,
			objectHeads(await current.storage.rebuild()),
		);
		if (changes.length <= OBJECT_EVENT_BATCH_LIMIT) {
			for (const change of changes)
				this.#emit(change.event, change.payload, source);
			return;
		}
		this.#emit(
			'objects:changed',
			{
				changes: changes.map((change) => ({
					...change.payload,
					event: change.event,
				})),
			},
			source,
		);
	}

	/** Tell workers in other tabs which files changed. */
	#broadcast(paths: string[] | null) {
		try {
			this.#current?.channel?.postMessage({
				type: 'workspace-changed',
				origin: this.#instance,
				paths: paths === null ? null : [...new Set(paths)],
			});
		} catch {
			// Other tabs still catch up on their next full read.
		}
	}

	/** Another tab changed files: refresh them and tell this tab's app. */
	async #receiveExternal(handle: WorkspaceHandle, paths: string[] | null) {
		if (this.#current !== handle) return;
		const before = objectHeads(await handle.storage.rebuild());
		await handle.storage.invalidate(paths ?? undefined);
		await this.#emitDiff(before, 'external');
		this.#emit(
			'file:changed',
			{ paths: paths ?? [], rescanned: paths === null },
			'external',
		);
		if (paths === null || paths.includes(MANIFEST_PATH))
			await this.#manifestChanged('external');
	}

	async #manifestChanged(source: EventSource) {
		const current = this.#current;
		if (!current) return;
		const manifest = await this.#readManifest(current.storage).catch(
			() => null,
		);
		if (!manifest) return;
		current.name = manifest.name;
		this.#emit(
			'workspace:manifest-updated',
			{ enabledPlugins: manifest.enabled_plugins, name: manifest.name },
			source,
		);
	}
}

/**
 * Installs a worker message boundary. Requests are isolated so concurrent
 * calls always settle.
 */
export function installBrowserWorkspaceWorker(
	port: BrowserWorkerPort,
	server: BrowserWorkspaceServer | Promise<BrowserWorkspaceServer>,
) {
	const ready = Promise.resolve(server);
	port.addEventListener('message', (event) => {
		const request = event.data as unknown;
		if (!isWorkerRequest(request)) {
			const id =
				isRecord(request) && typeof request.id === 'string' ? request.id : '';
			port.postMessage({
				type: 'response',
				id,
				ok: false,
				error: coreError(
					'invalid_worker_request',
					'validation',
					'The browser worker request has an invalid shape',
					'worker_request',
				),
			});
			return;
		}
		void ready
			.then((value) => value.request(request.command, request.payload))
			.then(
				(value) =>
					port.postMessage({
						type: 'response',
						id: request.id,
						ok: true,
						value,
					}),
				(error) =>
					port.postMessage({
						type: 'response',
						id: request.id,
						ok: false,
						error: asCoreError(error, request.command),
					}),
			);
	});
}

/** Creates the OPFS registry used by the real browser worker. */
export async function createOpfsWorkspaceRegistry(
	format: WorkspaceFormat,
): Promise<BrowserWorkspaceRegistry> {
	const storage = globalThis.navigator?.storage;
	if (!storage || typeof storage.getDirectory !== 'function')
		throw new BrowserStorageError(
			'unsupported',
			'This browser does not provide the Origin Private File System',
		);
	const root = await storage.getDirectory();
	let collection = root;
	for (const directory of COLLECTION_DIRECTORY.split('/'))
		collection = await collection.getDirectoryHandle(directory, {
			create: true,
		});
	const open = async (id: string, create: boolean) => {
		if (!format.isValidObjectId(id, 'workspace'))
			throw new BrowserStorageError(
				'invalid_path',
				'The browser workspace ID is invalid',
			);
		try {
			const directory = await collection.getDirectoryHandle(id, { create });
			const storage = new BrowserWorkspaceStorage({
				fileSystem: new OpfsFileSystem(directory),
				format,
				lock: createBrowserStorageLock(`noura:browser-workspace:${id}`),
			});
			await storage.recover();
			return storage;
		} catch (error) {
			if (error instanceof DOMException && error.name === 'NotFoundError')
				return null;
			throw error;
		}
	};
	return {
		async createFresh(id) {
			if (!format.isValidObjectId(id, 'workspace'))
				throw new BrowserStorageError(
					'invalid_path',
					'The browser workspace ID is invalid',
				);
			const registryLock = createBrowserStorageLock(
				'noura:browser-workspace-registry',
			);
			return registryLock.run(async () => {
				try {
					await collection.getDirectoryHandle(id);
					throw new BrowserStorageError(
						'workspace_not_empty',
						'A browser workspace with this stable ID already exists',
					);
				} catch (error) {
					if (
						!(error instanceof DOMException) ||
						error.name !== 'NotFoundError'
					)
						throw error;
				}
				return (await open(id, true))!;
			});
		},
		open: (id) => open(id, false),
		async list() {
			const result: Array<{ id: string; storage: BrowserWorkspaceStorage }> =
				[];
			for await (const handle of (
				collection as FileSystemDirectoryHandle & {
					values(): AsyncIterable<FileSystemHandle>;
				}
			).values()) {
				if (
					handle.kind !== 'directory' ||
					!format.isValidObjectId(handle.name, 'workspace')
				)
					continue;
				const workspace = await open(handle.name, false);
				if (workspace) result.push({ id: handle.name, storage: workspace });
			}
			return result;
		},
	};
}

/**
 * Starts the complete OPFS and canonical-WASM worker runtime. Requests that
 * arrive while it starts wait for it; if it cannot start, each request fails
 * with `browser_storage_unavailable`. The port also receives one `ready` or
 * `startup-error` message.
 */
export function startBrowserWorkspaceWorker(
	port: BrowserWorkerPort & {
		postMessage(
			message:
				| BrowserWorkerResponse
				| { type: 'ready' }
				| { type: 'startup-error'; stage: string },
		): void;
	},
	options: { wasmUrl?: string } = {},
): Promise<void> {
	let stage = 'wasm';
	const server = (async () => {
		const format = await loadWorkspaceFormat(options.wasmUrl);
		stage = 'storage';
		const registry = await createOpfsWorkspaceRegistry(format);
		return new BrowserWorkspaceServer({
			format,
			registry,
			onEvent: (event) => port.postMessage({ type: 'event', event }),
		});
	})();
	const guarded = server.catch((): never => {
		throw coreError(
			'browser_storage_unavailable',
			'filesystem',
			'This browser can’t store workspaces. Use a current browser over a secure connection.',
			'worker_start',
		);
	});
	installBrowserWorkspaceWorker(port, guarded);
	return server.then(
		() => port.postMessage({ type: 'ready' }),
		() => port.postMessage({ type: 'startup-error', stage }),
	);
}

function mutation(value: WorkspaceObject): MutationResult<WorkspaceObject> {
	return {
		value,
		revision: value.revision,
		durability: 'committed',
		indexStatus: 'updated',
		warnings: [],
	};
}
function objectEventPayload(value: WorkspaceObject) {
	return {
		id: value.id,
		type: value.type,
		path: value.relativePath,
		revision: value.revision,
	};
}
function supportedType(value: unknown): value is ManagedType {
	return value === 'note' || value === 'task' || value === 'project';
}
function managedType(value: unknown, operation: string): ManagedType {
	if (!supportedType(value))
		throw coreError(
			'browser_operation_unsupported',
			'validation',
			'Browser workspaces support note, task, and project objects only',
			operation,
		);
	return value;
}
function validPath(path: string, operation: string): string {
	try {
		return validateRelativePath(path);
	} catch {
		throw coreError(
			'invalid_path',
			'validation',
			'The path is not a valid workspace path',
			operation,
		);
	}
}
function baseNameOf(path: string): string {
	return path.slice(path.lastIndexOf('/') + 1);
}
function fileStem(path: string): string {
	const name = baseNameOf(path);
	const dot = name.lastIndexOf('.');
	return dot > 0 ? name.slice(0, dot) : name;
}
function normalizedProperties(
	properties: Record<string, unknown>,
): Record<string, unknown> {
	return Object.fromEntries(
		Object.entries(properties).filter(([key]) => !RESERVED_PROPERTIES.has(key)),
	);
}
/** Structural equality of JSON values, ignoring key order. */
function sameJson(left: unknown, right: unknown): boolean {
	if (left === right) return true;
	if (
		typeof left !== 'object' ||
		typeof right !== 'object' ||
		left === null ||
		right === null ||
		Array.isArray(left) !== Array.isArray(right)
	)
		return false;
	const leftKeys = Object.keys(left);
	const rightKeys = Object.keys(right);
	return (
		leftKeys.length === rightKeys.length &&
		leftKeys.every(
			(key) =>
				Object.hasOwn(right, key) &&
				sameJson(
					(left as Record<string, unknown>)[key],
					(right as Record<string, unknown>)[key],
				),
		)
	);
}
function objectNotFound(operation: string): CoreError {
	return coreError(
		'object_not_found',
		'validation',
		'The object does not exist',
		operation,
	);
}
function fileNotFound(operation: string): CoreError {
	return coreError(
		'file_not_found',
		'filesystem',
		'The file does not exist',
		operation,
	);
}
function revisionConflict(operation: string): CoreError {
	return coreError(
		'revision_conflict',
		'conflict',
		'The file changed before the mutation could be committed',
		operation,
	);
}
function typeMismatch(operation: string): CoreError {
	return coreError(
		'object_type_mismatch',
		'validation',
		'Only note drafts can be reconciled',
		operation,
	);
}
function changedDuringReview(operation: string, revision: string): CoreError {
	return {
		...coreError(
			'revision_conflict',
			'conflict',
			'The file changed again while the conflict was being reviewed',
			operation,
		),
		details: { currentRevision: revision },
	} as CoreError;
}
async function readIfValid(storage: BrowserWorkspaceStorage, path: string) {
	try {
		return await storage.read(path);
	} catch (error) {
		if (error instanceof BrowserStorageError) return null;
		throw error;
	}
}
/** The resolvable path of an Obsidian-style target, like native. */
function markdownTargetPath(target: string, allowFragment: boolean): string {
	const path = (target.split('|')[0] ?? '').trim();
	return allowFragment ? (path.split('#')[0] ?? '') : path;
}
function invalidTarget(message: string): CoreError {
	return coreError(
		'invalid_markdown_target',
		'validation',
		message,
		'markdown_target_resolve',
	);
}
/** Native `pdf::decode_target`: strict percent-decoding of a PDF link. */
function decodeTarget(target: string): string {
	const bytes: number[] = [];
	const source = new TextEncoder().encode(target);
	for (let index = 0; index < source.length; index += 1) {
		const byte = source[index]!;
		if (byte !== 0x25) {
			bytes.push(byte);
			continue;
		}
		const digits = String.fromCharCode(
			source[index + 1] ?? 0,
			source[index + 2] ?? 0,
		);
		if (!/^[0-9a-fA-F]{2}$/u.test(digits))
			throw invalidTarget('Invalid URL escape in file link');
		bytes.push(Number.parseInt(digits, 16));
		index += 2;
	}
	let value: string;
	try {
		value = new TextDecoder('utf-8', { fatal: true }).decode(
			new Uint8Array(bytes),
		);
	} catch {
		throw invalidTarget('Invalid URL escape in file link');
	}
	if (value.includes('\0'))
		throw invalidTarget('Invalid URL escape in file link');
	return value;
}
/** Native `resolve_markdown_target`: a link resolved from its note's folder. */
function resolveMarkdownTarget(
	source: string,
	rawTarget: string,
	allowFragment: boolean,
): string {
	const operation = 'markdown_target_resolve';
	let target = markdownTargetPath(rawTarget, allowFragment);
	if (target.length === 0 || target.includes('\0'))
		throw invalidTarget('The Markdown target is empty or invalid');
	if (target.toLowerCase().endsWith('.pdf')) target = decodeTarget(target);
	else {
		try {
			const decoded = decodeTarget(target);
			if (decoded.toLowerCase().endsWith('.pdf')) target = decoded;
		} catch {
			// Only PDF links are percent-decoded.
		}
	}
	if (target.startsWith('/') || /^[A-Za-z]:/u.test(target))
		throw invalidTarget(
			'The Markdown target must be a relative workspace path',
		);
	validPath(source, operation);
	const parts = source.split('/').slice(0, -1);
	for (const part of target.split('/')) {
		if (part === '' || part === '.') continue;
		if (part === '..') {
			if (parts.pop() === undefined)
				throw coreError(
					'path_traversal',
					'validation',
					'The Markdown target escapes the workspace',
					operation,
				);
			continue;
		}
		parts.push(part);
	}
	return validPath(parts.join('/'), operation);
}

function validateSnapshot(
	payload: Record<string, unknown>,
	format: WorkspaceFormat,
): {
	snapshot: BrowserWorkspaceSnapshot;
	manifest: { id: string; name: string };
} {
	if (!hasOnlyKeys(payload, ['snapshot']) || !isRecord(payload.snapshot))
		return invalidSnapshot();
	const value = payload.snapshot;
	if (
		!hasOnlyKeys(value, ['format', 'version', 'workspaceId', 'entries']) ||
		value.format !== SNAPSHOT_FORMAT ||
		value.version !== 1 ||
		typeof value.workspaceId !== 'string' ||
		!format.isValidObjectId(value.workspaceId, 'workspace') ||
		!Array.isArray(value.entries) ||
		value.entries.length === 0 ||
		value.entries.length > BROWSER_SNAPSHOT_LIMITS.maxEntries
	)
		return invalidSnapshot();

	const paths = new Set<string>();
	let totalBytes = 0;
	const entries = value.entries.map((entry): BrowserWorkspaceSnapshotEntry => {
		if (
			!isRecord(entry) ||
			!hasOnlyKeys(entry, ['path', 'bytes']) ||
			typeof entry.path !== 'string' ||
			!(entry.bytes instanceof Uint8Array)
		)
			return invalidSnapshot();
		let path: string;
		try {
			path = validateRelativePath(entry.path);
		} catch {
			return invalidSnapshot();
		}
		if (paths.has(path) || isExcludedFromSnapshot(path))
			return invalidSnapshot();
		paths.add(path);
		if (entry.bytes.byteLength > BROWSER_SNAPSHOT_LIMITS.maxEntryBytes)
			return invalidSnapshot();
		totalBytes += entry.bytes.byteLength;
		if (totalBytes > BROWSER_SNAPSHOT_LIMITS.maxTotalBytes)
			return invalidSnapshot();
		return { path, bytes: entry.bytes.slice() };
	});
	const manifestEntry = entries.find((entry) => entry.path === MANIFEST_PATH);
	if (!manifestEntry) return invalidSnapshot();
	const manifest = format.parseWorkspaceManifest(manifestEntry.bytes);
	if (manifest.id !== value.workspaceId)
		throw coreError(
			'workspace_identity_mismatch',
			'identity',
			'The snapshot manifest does not match its stable workspace ID',
			'workspace_import',
		);
	return {
		snapshot: {
			format: SNAPSHOT_FORMAT,
			version: 1,
			workspaceId: value.workspaceId,
			entries,
		},
		manifest,
	};
}
function invalidSnapshot(): never {
	throw coreError(
		'invalid_workspace_snapshot',
		'validation',
		'The browser workspace snapshot has an invalid shape or file entry',
		'workspace_import',
	);
}
function record(value: unknown, operation: string): Record<string, unknown> {
	if (!value || typeof value !== 'object' || Array.isArray(value))
		throw coreError(
			'invalid_input',
			'validation',
			'The request input has an invalid shape',
			operation,
		);
	return value as Record<string, unknown>;
}
function string(value: unknown, name: string, operation: string): string {
	if (typeof value !== 'string')
		throw coreError(
			'invalid_input',
			'validation',
			`${name} must be a string`,
			operation,
		);
	return value;
}
function optionalString(
	value: unknown,
	name: string,
	operation: string,
): string | undefined {
	if (value === undefined) return undefined;
	return string(value, name, operation);
}
function optionalNullableString(
	value: unknown,
	name: string,
	operation: string,
): string | undefined {
	if (value === undefined || value === null) return undefined;
	return string(value, name, operation);
}
function optionalNullableStrings(
	value: unknown,
	name: string,
	operation: string,
): string[] | undefined {
	if (value === undefined || value === null) return undefined;
	return strings(value, name, operation);
}
function strings(value: unknown, name: string, operation: string): string[] {
	if (!Array.isArray(value) || value.some((item) => typeof item !== 'string'))
		throw coreError(
			'invalid_input',
			'validation',
			`${name} must be an array of strings`,
			operation,
		);
	return value;
}
function coreError(
	code: string,
	category: CoreError['category'],
	message: string,
	operation: string,
): CoreError {
	return { code, category, message, retryable: false, operation };
}
function requiredRevision(
	value: unknown,
	name: string,
	operation: string,
): string {
	return string(value, name, operation);
}
function nullableRevision(
	value: unknown,
	name: string,
	operation: string,
): string | null {
	if (value === undefined || value === null) return null;
	return string(value, name, operation);
}
function encodeBase64Bytes(bytes: Uint8Array): string {
	let binary = '';
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return btoa(binary);
}
function decodeBase64Bytes(
	value: unknown,
	name: string,
	operation: string,
): Uint8Array {
	const text = string(value, name, operation);
	let binary: string;
	try {
		binary = atob(text);
	} catch {
		throw coreError(
			'invalid_base64',
			'validation',
			`${name} must be canonical base64`,
			operation,
		);
	}
	const bytes = new Uint8Array(binary.length);
	for (let index = 0; index < binary.length; index += 1)
		bytes[index] = binary.charCodeAt(index);
	if (encodeBase64Bytes(bytes) !== text)
		throw coreError(
			'invalid_base64',
			'validation',
			`${name} must be canonical base64`,
			operation,
		);
	return bytes;
}
function asCoreError(
	error: unknown,
	operation: string,
	workspaceId?: string,
): CoreError {
	if (isCoreError(error)) return error;
	if (error instanceof BrowserStorageError) {
		const category =
			error.code === 'stale_revision' || error.code === 'path_exists'
				? 'conflict'
				: error.code === 'unsupported'
					? 'filesystem'
					: 'validation';
		return withWorkspace(
			coreError(
				error.code === 'stale_revision' ? 'revision_conflict' : error.code,
				category,
				error.message,
				operation,
			),
			workspaceId,
		);
	}
	const format = error as WorkspaceFormatError;
	if (format?.name === 'WorkspaceFormatError')
		return withWorkspace(
			coreError(format.code, 'validation', format.message, operation),
			workspaceId,
		);
	return withWorkspace(
		coreError(
			'browser_workspace_failed',
			'filesystem',
			'The browser workspace operation failed',
			operation,
		),
		workspaceId,
	);
}
function withWorkspace(
	error: CoreError,
	workspaceId: string | undefined,
): CoreError {
	return workspaceId === undefined ? error : { ...error, workspaceId };
}
function isCoreError(value: unknown): value is CoreError {
	return (
		typeof value === 'object' &&
		value !== null &&
		'code' in value &&
		'category' in value &&
		'operation' in value
	);
}
function isWorkerRequest(value: unknown): value is BrowserWorkerRequest {
	return (
		isRecord(value) &&
		value.type === 'request' &&
		typeof value.id === 'string' &&
		typeof value.command === 'string' &&
		isRecord(value.payload)
	);
}
function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function hasOnlyKeys(
	value: Record<string, unknown>,
	keys: readonly string[],
): boolean {
	return Object.keys(value).every((key) => keys.includes(key));
}
function browserWorkspaceId(path: string, format: WorkspaceFormat): string {
	const id = path.slice(BROWSER_PATH_PREFIX.length);
	if (
		!path.startsWith(BROWSER_PATH_PREFIX) ||
		!format.isValidObjectId(id, 'workspace')
	)
		throw coreError(
			'invalid_browser_path',
			'validation',
			'Browser workspace paths use browser://<workspace-id>',
			'workspace_open',
		);
	return id;
}
