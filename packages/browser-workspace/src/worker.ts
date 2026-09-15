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
	validateRelativePath,
} from '@noura/browser-storage';
import type {
	CoreError,
	CoreEvent,
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

const MANIFEST_PATH = '.noura/workspace.yaml';
const COLLECTION_DIRECTORY = '.noura/browser-workspaces';
const BROWSER_PATH_PREFIX = 'browser://';
const SNAPSHOT_FORMAT = 'noura.workspace-snapshot';

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

type WorkspaceHandle = {
	id: string;
	name: string;
	storage: BrowserWorkspaceStorage;
};

type BrowserObject = WorkspaceObject & { type: 'note' | 'task' | 'project' };

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
};

/** Routes the supported browser workspace subset; every other Core command fails explicitly. */
export class BrowserWorkspaceServer {
	readonly #format: WorkspaceFormat;
	readonly #registry: BrowserWorkspaceRegistry;
	readonly #now: () => string;
	readonly #onEvent: ((event: CoreEvent) => void) | undefined;
	#current: WorkspaceHandle | null = null;
	#requests: Promise<void> = Promise.resolve();

	constructor({
		format,
		registry,
		now = () => new Date().toISOString(),
		onEvent,
	}: BrowserWorkspaceServerOptions) {
		this.#format = format;
		this.#registry = registry;
		this.#now = now;
		this.#onEvent = onEvent;
	}

	request(
		command: string,
		payload: Record<string, unknown> = {},
	): Promise<unknown> {
		const operation = async () => {
			try {
				switch (command) {
					case 'workspace_create':
						return await this.#createWorkspace(payload);
					case 'workspace_open':
						return await this.#openWorkspace(payload);
					case 'workspace_close':
						this.#current = null;
						return undefined;
					case 'workspace_state':
						return await this.#state();
					case 'workspace_list_recent':
						return await this.#listWorkspaces();
					case 'workspace_export':
						return await this.#exportWorkspace();
					case 'workspace_import':
						return await this.#importWorkspace(payload);
					case 'workspace_rebuild_index':
						return await this.#rebuild();
					case 'manifest_read':
						return await this.#manifest();
					case 'manifest_update':
						return await this.#updateManifest(payload);
					case 'objects_query':
						return await this.#listObjects(payload);
					case 'objects_list':
						return await this.#listObjectCards();
					case 'objects_get':
						return await this.#getObject(payload);
					case 'objects_create':
						return await this.#createObject(payload);
					case 'objects_update':
						return await this.#updateObject(payload);
					case 'objects_move':
						return await this.#moveObject(payload);
					case 'objects_delete':
						return await this.#deleteObject(payload);
					case 'files_list':
						return await this.#listFiles();
					case 'files_read':
						return await this.#readFile(payload);
					case 'files_write':
						return await this.#writeFile(payload);
					case 'files_move':
						return await this.#moveFile(payload);
					case 'files_delete':
						return await this.#deleteFile(payload);
					default:
						throw coreError(
							'browser_operation_unsupported',
							'validation',
							`The browser workspace does not support ${command}`,
							command,
						);
				}
			} catch (error) {
				throw asCoreError(error, command, this.#current?.id);
			}
		};
		const result = this.#requests.then(operation, operation);
		this.#requests = result.then(
			() => undefined,
			() => undefined,
		);
		return result;
	}

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
		const manifest = this.#format.createWorkspaceManifest(name, this.#now());
		const storage = await this.#registry.createFresh(manifest.id);
		const bytes = new TextEncoder().encode(
			this.#format.serializeWorkspaceManifest(manifest),
		);
		await storage.write({ path: MANIFEST_PATH, bytes, expectedRevision: null });
		this.#current = { id: manifest.id, name: manifest.name, storage };
		this.#emit('workspace:opened');
		return this.#state();
	}

	async #openWorkspace(
		payload: Record<string, unknown>,
	): Promise<WorkspaceState> {
		const input = record(payload.input, 'workspace_open');
		const id = browserWorkspaceId(
			string(input.path, 'path', 'workspace_open'),
			this.#format,
		);
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
		this.#current = { id: manifest.id, name: manifest.name, storage };
		this.#emit('workspace:opened');
		return this.#state();
	}

	async #state(): Promise<WorkspaceState> {
		if (!this.#current)
			return { phase: 'idle', indexedFiles: 0, diagnostics: [] };
		const rebuilt = await this.#current.storage.rebuild();
		return {
			phase: 'ready',
			workspaceId: this.#current.id,
			rootPath: `${BROWSER_PATH_PREFIX}${this.#current.id}`,
			indexedFiles: rebuilt.files.length,
			diagnostics: rebuilt.malformedMarkdown.map((value) => ({
				code: 'malformed_markdown',
				message: value.error,
				relativePath: value.path,
			})),
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
		this.#current = { id: manifest.id, name: manifest.name, storage };
		this.#emit('workspace:opened');
		return this.#state();
	}

	async #rebuild(): Promise<WorkspaceState> {
		this.#requireCurrent('workspace_rebuild_index');
		return this.#state();
	}

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

	async #listObjects(
		payload: Record<string, unknown>,
	): Promise<WorkspaceObject[]> {
		const query =
			payload.query === undefined ? {} : record(payload.query, 'objects_query');
		if (query.type !== undefined && !supportedObjectType(query.type))
			throw coreError(
				'browser_operation_unsupported',
				'validation',
				'Browser workspaces support note, task, and project objects only',
				'objects_query',
			);
		const objects = (
			await this.#requireCurrent('objects_query').storage.rebuild()
		).managed.filter(
			(value) => supportedObjectType(value.type) && matchesQuery(value, query),
		);
		return objects;
	}

	/**
	 * Minimal managed-object projection for sync bindings. Unlike
	 * `objects_query` it returns no bodies or properties, so a binding can own an
	 * object without reading workspace content through the worker.
	 */
	async #listObjectCards(): Promise<
		Array<{ id: string; path: string; type: string }>
	> {
		const managed = (
			await this.#requireCurrent('objects_list').storage.rebuild()
		).managed.filter((value) => supportedObjectType(value.type));
		return managed
			.map((value) => ({
				id: value.id,
				path: value.relativePath,
				type: value.type,
			}))
			.sort((left, right) => left.path.localeCompare(right.path));
	}

	async #getObject(payload: Record<string, unknown>): Promise<WorkspaceObject> {
		const id = string(payload.id, 'id', 'objects_get');
		const object = await this.#findObject(id, 'objects_get');
		if (!object)
			throw coreError(
				'object_not_found',
				'validation',
				'The object does not exist',
				'objects_get',
			);
		return object;
	}

	async #createObject(payload: Record<string, unknown>) {
		const input = record(payload.input, 'objects_create');
		if (!supportedObjectType(input.type))
			throw coreError(
				'browser_operation_unsupported',
				'validation',
				'Browser workspaces support note, task, and project objects only',
				'objects_create',
			);
		const relativePath = optionalNullableString(
			input.relativePath,
			'relativePath',
			'objects_create',
		);
		if (
			relativePath !== undefined &&
			!this.#format.isValidManagedObjectPath(relativePath)
		)
			throw coreError(
				'invalid_path',
				'validation',
				'The managed object destination is invalid',
				'objects_create',
			);
		const objectInput = {
			title: string(input.title, 'title', 'objects_create'),
			body: typeof input.body === 'string' ? input.body : '',
			...(relativePath === undefined ? {} : { relativePath }),
			properties:
				input.properties === undefined
					? {}
					: record(input.properties, 'objects_create'),
			now: this.#now(),
		};
		const object =
			input.type === 'task'
				? this.#format.createTask(objectInput)
				: input.type === 'project'
					? this.#format.createProject(objectInput)
					: this.#format.createNote(objectInput);
		const storage = this.#requireCurrent('objects_create').storage;
		const stored = await storage.write({
			path: object.relativePath,
			bytes: this.#format.serializeObject(object),
			expectedRevision: null,
		});
		const value = this.#managed(
			object.relativePath,
			stored.bytes,
			input.type,
			'objects_create',
		);
		this.#emit('object:created', objectEventPayload(value));
		return mutation(value);
	}

	async #updateObject(payload: Record<string, unknown>) {
		const id = string(payload.id, 'id', 'objects_update');
		const patch = record(payload.patch, 'objects_update');
		const current = await this.#findObject(id, 'objects_update');
		if (!current)
			throw coreError(
				'object_not_found',
				'validation',
				'The object does not exist',
				'objects_update',
			);
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
		const updateInput = {
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
			now: this.#now(),
		};
		const object =
			current.type === 'task'
				? this.#format.updateTask(current, updateInput)
				: current.type === 'project'
					? this.#format.updateProject(current, updateInput)
					: this.#format.updateNote(current, updateInput);
		const stored = await this.#requireCurrent('objects_update').storage.write({
			path: current.relativePath,
			bytes: this.#format.serializeObject(object),
			expectedRevision,
		});
		const value = this.#managed(
			current.relativePath,
			stored.bytes,
			current.type,
			'objects_update',
		);
		this.#emit('object:updated', objectEventPayload(value));
		return mutation(value);
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
		const current = await this.#findObject(id, 'objects_move');
		if (!current)
			throw coreError(
				'object_not_found',
				'validation',
				'The object does not exist',
				'objects_move',
			);
		const stored = await this.#requireCurrent('objects_move').storage.move({
			from: current.relativePath,
			to: relativePath,
			expectedRevision,
			expectedDestinationRevision: null,
		});
		const value = this.#managed(
			relativePath,
			stored.bytes,
			current.type,
			'objects_move',
		);
		this.#emit('object:moved', {
			id: value.id,
			from: current.relativePath,
			to: relativePath,
		});
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
		const current = await this.#findObject(id, 'objects_delete');
		if (!current)
			throw coreError(
				'object_not_found',
				'validation',
				'The object does not exist',
				'objects_delete',
			);
		const trashPath = `.noura/trash/${trashTimestamp(this.#now())}/${current.relativePath}`;
		await this.#requireCurrent('objects_delete').storage.move({
			from: current.relativePath,
			to: trashPath,
			expectedRevision,
			expectedDestinationRevision: null,
		});
		this.#emit('object:deleted', {
			id: current.id,
			path: current.relativePath,
			trashPath,
		});
		return mutation(current);
	}

	/**
	 * Raw canonical filesystem operations. These are transport-level primitives
	 * over `BrowserWorkspaceStorage`, which owns path validation, expected
	 * revisions, journaling, and recovery. They perform no workspace-format
	 * interpretation and are the boundary a sync adapter uses to read and apply
	 * canonical bytes.
	 */
	async #listFiles(): Promise<string[]> {
		const rebuilt = await this.#requireCurrent('files_list').storage.rebuild();
		const paths = new Set<string>();
		for (const file of rebuilt.files) paths.add(file.path);
		for (const managed of rebuilt.managed) paths.add(managed.relativePath);
		return [...paths].sort();
	}

	async #readFile(payload: Record<string, unknown>) {
		if (!hasOnlyKeys(payload, ['path']))
			throw coreError(
				'invalid_input',
				'validation',
				'files_read accepts only a path',
				'files_read',
			);
		const path = string(payload.path, 'path', 'files_read');
		const stored = await this.#requireCurrent('files_read').storage.read(path);
		if (!stored) return null;
		return {
			revision: stored.revision,
			bytes: encodeBase64Bytes(stored.bytes),
		};
	}

	async #writeFile(payload: Record<string, unknown>) {
		if (!hasOnlyKeys(payload, ['path', 'bytes', 'expectedRevision']))
			throw coreError(
				'invalid_input',
				'validation',
				'files_write accepts only path, bytes, and expectedRevision',
				'files_write',
			);
		const path = string(payload.path, 'path', 'files_write');
		const bytes = decodeBase64Bytes(payload.bytes, 'bytes', 'files_write');
		const expectedRevision = nullableRevision(
			payload.expectedRevision,
			'expectedRevision',
			'files_write',
		);
		const stored = await this.#requireCurrent('files_write').storage.write({
			path,
			bytes,
			expectedRevision,
		});
		return { path, revision: stored.revision };
	}

	async #moveFile(payload: Record<string, unknown>) {
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
				'files_move',
			);
		const from = string(payload.from, 'from', 'files_move');
		const to = string(payload.to, 'to', 'files_move');
		const expectedRevision = requiredRevision(
			payload.expectedRevision,
			'expectedRevision',
			'files_move',
		);
		const expectedDestinationRevision = nullableRevision(
			payload.expectedDestinationRevision,
			'expectedDestinationRevision',
			'files_move',
		);
		const stored = await this.#requireCurrent('files_move').storage.move({
			from,
			to,
			expectedRevision,
			expectedDestinationRevision,
		});
		return { path: to, revision: stored.revision };
	}

	async #deleteFile(payload: Record<string, unknown>) {
		if (!hasOnlyKeys(payload, ['path', 'expectedRevision']))
			throw coreError(
				'invalid_input',
				'validation',
				'files_delete accepts only path and expectedRevision',
				'files_delete',
			);
		const path = string(payload.path, 'path', 'files_delete');
		const expectedRevision = requiredRevision(
			payload.expectedRevision,
			'expectedRevision',
			'files_delete',
		);
		await this.#requireCurrent('files_delete').storage.delete({
			path,
			expectedRevision,
		});
		return undefined;
	}

	async #findObject(
		id: string,
		operation: string,
	): Promise<BrowserObject | null> {
		const values = await this.#requireCurrent(operation).storage.rebuild();
		return (values.managed.find(
			(value) => value.id === id && supportedObjectType(value.type),
		) ?? null) as BrowserObject | null;
	}

	#managed(
		path: string,
		bytes: Uint8Array,
		expectedType: 'note' | 'task' | 'project',
		operation: string,
	): WorkspaceObject {
		const parsed = this.#format.parseMarkdown(path, bytes);
		if (parsed.kind !== 'managed' || parsed.type !== expectedType)
			throw coreError(
				'canonical_serialization_failed',
				'parse',
				'Canonical object bytes could not be read back',
				operation,
			);
		return parsed;
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

	#emit(type: string, payload: unknown = {}) {
		if (!this.#current) return;
		this.#onEvent?.({
			eventId: crypto.randomUUID(),
			type,
			workspaceId: this.#current.id,
			occurredAt: this.#now(),
			source: 'application',
			payload,
		});
	}
}

/** Installs a worker message boundary. Requests are isolated so concurrent calls always settle. */
export function installBrowserWorkspaceWorker(
	port: BrowserWorkerPort,
	server: BrowserWorkspaceServer,
) {
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
		void server.request(request.command, request.payload).then(
			(value) =>
				port.postMessage({ type: 'response', id: request.id, ok: true, value }),
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

/** Starts the complete OPFS and canonical-WASM worker runtime. */
export async function startBrowserWorkspaceWorker(port: BrowserWorkerPort) {
	const format = await loadWorkspaceFormat();
	const registry = await createOpfsWorkspaceRegistry(format);
	const server = new BrowserWorkspaceServer({
		format,
		registry,
		onEvent: (event) => port.postMessage({ type: 'event', event }),
	});
	installBrowserWorkspaceWorker(port, server);
}

function mutation(value: WorkspaceObject) {
	return {
		value,
		revision: value.revision,
		durability: 'committed' as const,
		indexStatus: 'updated' as const,
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
function trashTimestamp(value: string): string {
	return value.replaceAll(':', '-').replaceAll('.', '-');
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
function matchesQuery(value: WorkspaceObject, query: Record<string, unknown>) {
	return (
		(query.pathPrefix === undefined ||
			(typeof query.pathPrefix === 'string' &&
				value.relativePath.startsWith(query.pathPrefix))) &&
		(query.project === undefined ||
			value.properties.project === query.project) &&
		(query.status === undefined || value.properties.status === query.status) &&
		(query.priority === undefined ||
			value.properties.priority === query.priority)
	);
}
function supportedObjectType(
	value: unknown,
): value is 'note' | 'task' | 'project' {
	return value === 'note' || value === 'task' || value === 'project';
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
	const identities = new Set<string>();
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
		if (isLiveWorkspaceObjectPath(path) && path.endsWith('.md')) {
			const parsed = format.parseMarkdown(path, entry.bytes);
			if (parsed.kind === 'managed') {
				if (identities.has(parsed.id))
					throw coreError(
						'duplicate_object_identity',
						'identity',
						'The snapshot contains more than one canonical file for a stable object ID',
						'workspace_import',
					);
				identities.add(parsed.id);
			}
		}
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
