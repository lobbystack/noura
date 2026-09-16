import { describe, expect, test } from 'bun:test';
import {
	BROWSER_EXPORT_SNAPSHOT_LIMITS,
	BrowserStorageError,
	BrowserWorkspaceStorage,
	type BrowserStorageFileSystem,
	type BrowserStorageLock,
} from '@noura/browser-storage';
import type {
	CreateNoteInput,
	UpdateNoteInput,
	WorkspaceFormat,
	WorkspaceManifest,
	WorkspaceObject,
} from '@noura/workspace-format-wasm';
import { WorkspaceFormatError } from '@noura/workspace-format-wasm';
import { createBrowserWorkerTransport } from './transport';
import type {
	BrowserWorkerEndpoint,
	BrowserWorkerRequest,
	BrowserWorkerResponse,
} from './protocol';
import {
	BrowserWorkspaceServer,
	type BrowserWorkspaceRegistry,
	type BrowserWorkspaceSnapshot,
} from './worker';

const encoder = new TextEncoder();
const decoder = new TextDecoder();

class MemoryFileSystem implements BrowserStorageFileSystem {
	readonly files = new Map<string, Uint8Array>();
	async read(path: string, maxBytes?: number) {
		const value = this.files.get(path);
		if (value && maxBytes !== undefined && value.byteLength > maxBytes)
			throw new BrowserStorageError(
				'snapshot_too_large',
				'file would exceed the requested read limit',
			);
		return value?.slice() ?? null;
	}
	async write(path: string, bytes: Uint8Array) {
		this.files.set(path, bytes.slice());
	}
	async remove(path: string) {
		return this.files.delete(path);
	}
	async list(path: string) {
		const prefix = path.length === 0 ? '' : `${path}/`;
		const entries = new Map<string, 'file' | 'directory'>();
		for (const key of this.files.keys()) {
			if (!key.startsWith(prefix)) continue;
			const rest = key.slice(prefix.length);
			const slash = rest.indexOf('/');
			entries.set(
				slash === -1 ? rest : rest.slice(0, slash),
				slash === -1 ? 'file' : 'directory',
			);
		}
		return [...entries].map(([name, kind]) => ({ name, kind }));
	}
}

const lock: BrowserStorageLock = { run: (operation) => operation() };
let nextNote = 0;
const format: WorkspaceFormat = {
	contentRevision: (bytes) => `r:${decoder.decode(bytes)}`,
	isValidObjectId: () => true,
	isValidManagedObjectPath: (path) =>
		path.endsWith('.md') &&
		!path.startsWith('.') &&
		!path.startsWith('/') &&
		!path.includes('..') &&
		!path.includes('\\'),
	serializeWorkspaceManifest: (manifest) => JSON.stringify(manifest),
	parseWorkspaceManifest: (bytes) =>
		JSON.parse(decoder.decode(bytes)) as WorkspaceManifest,
	createWorkspaceManifest: (name, now) => ({
		id: 'workspace_01j00000000000000000000000',
		format_version: 1,
		name,
		created: now,
		updated: now,
		enabled_plugins: [],
		ignore: [],
	}),
	updateWorkspaceManifest: (manifest, input, now) => ({
		...manifest,
		...(input.name == null ? {} : { name: input.name.trim() }),
		...(input.enabledPlugins == null
			? {}
			: { enabled_plugins: [...new Set(input.enabledPlugins)].sort() }),
		...(input.ignore == null ? {} : { ignore: input.ignore }),
		updated: now,
	}),
	createNote: (input: CreateNoteInput) =>
		object({
			id: `note_01j0000000000000000000000${nextNote++}`.slice(0, 31),
			title: input.title,
			body: input.body ?? '',
			relativePath: input.relativePath ?? `notes/${nextNote}.md`,
			created: input.now,
			updated: input.now,
			properties: input.properties ?? {},
		}),
	createTask: (input: CreateNoteInput) =>
		object({
			id: `task_01j0000000000000000000000${nextNote++}`.slice(0, 31),
			type: 'task',
			title: input.title,
			body: input.body ?? '',
			relativePath: input.relativePath ?? `tasks/${nextNote}.md`,
			created: input.now,
			updated: input.now,
			properties: {
				status: 'todo',
				priority: 'medium',
				...input.properties,
			},
		}),
	createProject: (input: CreateNoteInput) =>
		object({
			id: `project_01j000000000000000000000${nextNote++}`.slice(0, 34),
			type: 'project',
			title: input.title,
			body: input.body ?? '',
			relativePath: input.relativePath ?? `projects/${nextNote}/project.md`,
			created: input.now,
			updated: input.now,
			properties: { status: 'planned', ...input.properties },
		}),
	updateNote: (note: WorkspaceObject, input: UpdateNoteInput) =>
		object({
			...note,
			title: input.title ?? note.title,
			body: input.body ?? note.body,
			updated: input.now,
			properties: patchedProperties(note.properties, input),
		}),
	updateTask: (task: WorkspaceObject, input: UpdateNoteInput) =>
		object({
			...task,
			title: input.title ?? task.title,
			body: input.body ?? task.body,
			updated: input.now,
			properties: {
				status: 'todo',
				priority: 'medium',
				...patchedProperties(task.properties, input),
			},
		}),
	updateProject: (project: WorkspaceObject, input: UpdateNoteInput) =>
		object({
			...project,
			title: input.title ?? project.title,
			body: input.body ?? project.body,
			updated: input.now,
			properties: {
				status: 'planned',
				...patchedProperties(project.properties, input),
			},
		}),
	serializeObject: (value) => encoder.encode(JSON.stringify(value)),
	parseMarkdown: (relativePath, bytes) => {
		const value = JSON.parse(decoder.decode(bytes)) as WorkspaceObject;
		return {
			kind: 'managed' as const,
			...value,
			relativePath,
			revision: `r:${decoder.decode(bytes)}`,
		};
	},
};

function object(
	value: Partial<WorkspaceObject> &
		Pick<
			WorkspaceObject,
			| 'id'
			| 'title'
			| 'body'
			| 'relativePath'
			| 'created'
			| 'updated'
			| 'properties'
		>,
): WorkspaceObject {
	return { type: 'note', revision: '', ...value };
}

function patchedProperties(
	properties: Record<string, unknown>,
	input: UpdateNoteInput,
): Record<string, unknown> {
	const next = { ...properties };
	for (const key of input.removeProperties ?? []) delete next[key];
	return { ...next, ...input.properties };
}

function registry(): {
	registry: BrowserWorkspaceRegistry;
	fileSystem: MemoryFileSystem;
} {
	const fileSystem = new MemoryFileSystem();
	let storage: BrowserWorkspaceStorage | null = null;
	return {
		fileSystem,
		registry: {
			async createFresh() {
				if (storage)
					throw new BrowserStorageError(
						'workspace_not_empty',
						'A browser workspace with this stable ID already exists',
					);
				storage = new BrowserWorkspaceStorage({ fileSystem, format, lock });
				return storage;
			},
			async open() {
				return storage;
			},
			async list() {
				return storage
					? [{ id: 'workspace_01j00000000000000000000000', storage }]
					: [];
			},
		},
	};
}

describe('BrowserWorkspaceServer', () => {
	test('routes workspace and note operations through canonical bytes', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser notes' },
		});
		const created = (await server.request('objects_create', {
			input: {
				type: 'note',
				title: 'First',
				body: 'Stored in OPFS',
				relativePath: null,
			},
		})) as { value: WorkspaceObject };
		const notes = (await server.request('objects_query', {
			query: { type: 'note' },
		})) as WorkspaceObject[];

		expect(notes).toHaveLength(1);
		expect(notes[0]?.id).toBe(created.value.id);
		expect(
			decoder.decode(values.fileSystem.files.get(created.value.relativePath)),
		).toContain('Stored in OPFS');
	});

	test('returns structured unsupported and stale-revision errors', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser notes' },
		});
		const created = (await server.request('objects_create', {
			input: { type: 'note', title: 'First' },
		})) as { value: WorkspaceObject };
		values.fileSystem.files.set(
			created.value.relativePath,
			encoder.encode(
				JSON.stringify({ ...created.value, body: 'external edit' }),
			),
		);
		await expect(
			server.request('objects_update', {
				id: created.value.id,
				patch: { expectedRevision: created.value.revision, body: 'Local edit' },
			}),
		).rejects.toMatchObject({
			code: 'revision_conflict',
			category: 'conflict',
		});
	});

	test('persists canonical plugin preferences and rejects a stale manifest tab', async () => {
		const values = registry();
		let tick = 0;
		const now = () => `2026-09-12T00:00:0${tick++}Z`;
		const first = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now,
		});
		await first.request('workspace_create', {
			input: { path: 'browser://', name: 'Plugin preferences' },
		});
		const original = (await first.request(
			'manifest_read',
		)) as WorkspaceManifest;
		const second = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now,
		});
		await second.request('workspace_open', {
			input: { path: `browser://${original.id}` },
		});
		const updated = (await first.request('manifest_update', {
			input: {
				enabledPlugins: ['tasks', 'notes', 'tasks'],
				expectedUpdated: original.updated,
			},
		})) as WorkspaceManifest;
		expect(updated.enabled_plugins).toEqual(['notes', 'tasks']);
		await expect(
			second.request('manifest_update', {
				input: {
					enabledPlugins: ['projects'],
					expectedUpdated: original.updated,
				},
			}),
		).rejects.toMatchObject({
			code: 'manifest_conflict',
			category: 'conflict',
		});
		const reloaded = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now,
		});
		await reloaded.request('workspace_open', {
			input: { path: `browser://${original.id}` },
		});
		expect(
			((await reloaded.request('manifest_read')) as WorkspaceManifest)
				.enabled_plugins,
		).toEqual(['notes', 'tasks']);
		await expect(reloaded.request('ai_credential_set')).rejects.toMatchObject({
			code: 'browser_operation_unsupported',
			category: 'validation',
		});
	});

	test('seeds the default enabled plugins on a new workspace and round-trips them', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Seeded plugins' },
		});
		const manifest = (await server.request(
			'manifest_read',
		)) as WorkspaceManifest;
		expect(manifest.enabled_plugins).toEqual(['notes', 'projects', 'tasks']);

		const reopened = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await reopened.request('workspace_open', {
			input: { path: `browser://${manifest.id}` },
		});
		expect(
			((await reopened.request('manifest_read')) as WorkspaceManifest)
				.enabled_plugins,
		).toEqual(['notes', 'projects', 'tasks']);
	});

	test('disabling a seeded default plugin persists across a reopen', async () => {
		const values = registry();
		let tick = 0;
		const now = () => `2026-09-12T00:00:0${tick++}Z`;
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now,
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Disabled plugin' },
		});
		const original = (await server.request(
			'manifest_read',
		)) as WorkspaceManifest;
		const updated = (await server.request('manifest_update', {
			input: {
				enabledPlugins: ['notes', 'tasks'],
				expectedUpdated: original.updated,
			},
		})) as WorkspaceManifest;
		expect(updated.enabled_plugins).toEqual(['notes', 'tasks']);

		const reopened = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now,
		});
		await reopened.request('workspace_open', {
			input: { path: `browser://${original.id}` },
		});
		expect(
			((await reopened.request('manifest_read')) as WorkspaceManifest)
				.enabled_plugins,
		).toEqual(['notes', 'tasks']);
	});

	test('a coarse wall clock cannot let a stale tab overwrite a newer preference write', async () => {
		const values = registry();
		// `Date` has millisecond resolution, so a real browser clock can return
		// the same instant for two manifest writes in different tabs.
		const now = () => '2026-09-12T00:00:00Z';
		const first = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now,
		});
		await first.request('workspace_create', {
			input: { path: 'browser://', name: 'Coarse clock' },
		});
		const original = (await first.request(
			'manifest_read',
		)) as WorkspaceManifest;
		const second = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now,
		});
		await second.request('workspace_open', {
			input: { path: `browser://${original.id}` },
		});

		const committed = (await first.request('manifest_update', {
			input: {
				enabledPlugins: ['notes', 'tasks'],
				expectedUpdated: original.updated,
			},
		})) as WorkspaceManifest;
		expect(committed.updated).not.toBe(original.updated);

		await expect(
			second.request('manifest_update', {
				input: {
					enabledPlugins: ['notes', 'projects'],
					expectedUpdated: original.updated,
				},
			}),
		).rejects.toMatchObject({
			code: 'manifest_conflict',
			category: 'conflict',
		});

		const next = (await first.request('manifest_update', {
			input: {
				enabledPlugins: ['notes'],
				expectedUpdated: committed.updated,
			},
		})) as WorkspaceManifest;
		expect(next.updated).not.toBe(committed.updated);

		const reloaded = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now,
		});
		await reloaded.request('workspace_open', {
			input: { path: `browser://${original.id}` },
		});
		expect(
			((await reloaded.request('manifest_read')) as WorkspaceManifest)
				.enabled_plugins,
		).toEqual(['notes']);
	});

	test('supports task list, read, status update, and unknown task metadata', async () => {
		const values = registry();
		const events: Array<{ type: string; payload: unknown }> = [];
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
			onEvent: (event) => events.push(event),
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser tasks' },
		});
		const created = (await server.request('objects_create', {
			input: {
				type: 'task',
				title: 'Ship browser tasks',
				properties: { custom: 'kept' },
			},
		})) as { value: WorkspaceObject };
		const listed = (await server.request('objects_query', {
			query: { type: 'task', status: 'todo' },
		})) as WorkspaceObject[];
		expect(listed).toHaveLength(1);
		expect(listed[0]).toMatchObject({
			id: created.value.id,
			type: 'task',
			properties: { status: 'todo', priority: 'medium', custom: 'kept' },
		});
		const updated = (await server.request('objects_update', {
			id: created.value.id,
			patch: {
				expectedRevision: created.value.revision,
				properties: { status: 'done' },
			},
		})) as { value: WorkspaceObject };
		expect(updated.value.properties).toMatchObject({
			status: 'done',
			priority: 'medium',
			custom: 'kept',
		});
		const removed = (await server.request('objects_update', {
			id: created.value.id,
			patch: {
				expectedRevision: updated.value.revision,
				removeProperties: ['custom', 'priority'],
			},
		})) as { value: WorkspaceObject };
		expect(removed.value.properties).toEqual({
			status: 'done',
			priority: 'medium',
		});
		expect(events.at(-1)).toMatchObject({
			type: 'object:updated',
			payload: {
				id: created.value.id,
				type: 'task',
				path: created.value.relativePath,
				revision: removed.value.revision,
			},
		});
	});

	test('supports project lifecycle with stable task references and no task cascade', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser projects' },
		});
		const project = (await server.request('objects_create', {
			input: {
				type: 'project',
				title: 'Launch',
				properties: { custom: 'retained' },
			},
		})) as { value: WorkspaceObject };
		const task = (await server.request('objects_create', {
			input: {
				type: 'task',
				title: 'Ship',
				properties: { project: project.value.id },
			},
		})) as { value: WorkspaceObject };

		const listed = (await server.request('objects_query', {
			query: { type: 'project', status: 'planned' },
		})) as WorkspaceObject[];
		expect(listed).toEqual([
			expect.objectContaining({
				id: project.value.id,
				type: 'project',
				properties: { status: 'planned', custom: 'retained' },
			}),
		]);
		expect(
			await server.request('objects_query', {
				query: { type: 'task', project: project.value.id },
			}),
		).toEqual([expect.objectContaining({ id: task.value.id })]);
		const updated = (await server.request('objects_update', {
			id: project.value.id,
			patch: {
				expectedRevision: project.value.revision,
				properties: { status: 'active' },
			},
		})) as { value: WorkspaceObject };
		expect(updated.value.properties).toEqual({
			status: 'active',
			custom: 'retained',
		});
		const moved = (await server.request('objects_move', {
			input: {
				id: updated.value.id,
				relativePath: 'archive/launch/project.md',
				expectedRevision: updated.value.revision,
			},
		})) as { value: WorkspaceObject };
		expect(moved.value).toMatchObject({
			id: project.value.id,
			relativePath: 'archive/launch/project.md',
		});
		expect(await server.request('workspace_rebuild_index')).toMatchObject({
			phase: 'ready',
			indexedFiles: 3,
		});

		await server.request('objects_delete', {
			input: {
				id: moved.value.id,
				expectedRevision: moved.value.revision,
			},
		});
		expect(await server.request('objects_get', { id: task.value.id })).toEqual(
			expect.objectContaining({ id: task.value.id }),
		);
		const exported = (await server.request(
			'workspace_export',
		)) as BrowserWorkspaceSnapshot;
		expect(exported.entries).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					path: expect.stringMatching(
						/\.noura\/trash\/.*\/archive\/launch\/project\.md$/,
					),
				}),
			]),
		);
		const restoredValues = registry();
		const restored = new BrowserWorkspaceServer({
			format,
			registry: restoredValues.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await restored.request('workspace_import', { snapshot: exported });
		expect(
			await restored.request('objects_get', { id: task.value.id }),
		).toEqual(expect.objectContaining({ id: task.value.id }));
		expect(
			[...restoredValues.fileSystem.files.keys()].some((path) =>
				path.startsWith('.noura/trash/'),
			),
		).toBe(true);
	});

	test('moves notes without changing their canonical bytes or stable identity', async () => {
		const values = registry();
		const events: Array<{ type: string; payload: unknown }> = [];
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
			onEvent: (event) => events.push(event),
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser notes' },
		});
		const created = (await server.request('objects_create', {
			input: { type: 'note', title: 'First', body: 'Unchanged bytes' },
		})) as { value: WorkspaceObject };
		const before = values.fileSystem.files
			.get(created.value.relativePath)
			?.slice();

		const moved = (await server.request('objects_move', {
			input: {
				id: created.value.id,
				relativePath: 'archive/first.md',
				expectedRevision: created.value.revision,
			},
		})) as { value: WorkspaceObject; revision: string };

		expect(moved.value).toMatchObject({
			id: created.value.id,
			relativePath: 'archive/first.md',
		});
		expect(moved.revision).toBe(created.value.revision);
		expect(values.fileSystem.files.get('archive/first.md')).toEqual(before);
		expect(values.fileSystem.files.has(created.value.relativePath)).toBe(false);
		expect(events.at(-1)).toMatchObject({
			type: 'object:moved',
			payload: {
				id: created.value.id,
				from: created.value.relativePath,
				to: 'archive/first.md',
			},
		});
	});

	test('rejects malformed destinations and stale revisions without moving the note', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser notes' },
		});
		const created = (await server.request('objects_create', {
			input: { type: 'note', title: 'First' },
		})) as { value: WorkspaceObject };
		await expect(
			server.request('objects_move', {
				input: {
					id: created.value.id,
					relativePath: '.noura/hidden.md',
					expectedRevision: created.value.revision,
				},
			}),
		).rejects.toMatchObject({ code: 'invalid_path' });
		values.fileSystem.files.set(
			created.value.relativePath,
			encoder.encode(
				JSON.stringify({ ...created.value, body: 'external edit' }),
			),
		);
		await expect(
			server.request('objects_move', {
				input: {
					id: created.value.id,
					relativePath: 'archive/first.md',
					expectedRevision: created.value.revision,
				},
			}),
		).rejects.toMatchObject({ code: 'revision_conflict' });
		expect(values.fileSystem.files.has('archive/first.md')).toBe(false);
	});

	test('rejects invalid create destinations before writing canonical bytes', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser notes' },
		});

		await expect(
			server.request('objects_create', {
				input: {
					type: 'note',
					title: 'Internal overwrite',
					relativePath: '.noura/workspace.yaml',
				},
			}),
		).rejects.toMatchObject({ code: 'invalid_path' });
		expect(values.fileSystem.files.has('.noura/workspace.yaml')).toBe(true);
	});

	test('rejects an occupied destination without overwriting its canonical bytes', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser notes' },
		});
		const created = (await server.request('objects_create', {
			input: { type: 'note', title: 'First' },
		})) as { value: WorkspaceObject };
		const occupied = (await server.request('objects_create', {
			input: {
				type: 'note',
				title: 'Occupied',
				relativePath: 'archive/first.md',
			},
		})) as { value: WorkspaceObject };
		const occupiedBytes = values.fileSystem.files
			.get(occupied.value.relativePath)
			?.slice();

		await expect(
			server.request('objects_move', {
				input: {
					id: created.value.id,
					relativePath: 'archive/first.md',
					expectedRevision: created.value.revision,
				},
			}),
		).rejects.toMatchObject({ code: 'path_exists', category: 'conflict' });
		expect(values.fileSystem.files.get('archive/first.md')).toEqual(
			occupiedBytes,
		);
		expect(values.fileSystem.files.has(created.value.relativePath)).toBe(true);
	});

	test('trashes notes durably and retains trash metadata in exports', async () => {
		const values = registry();
		const events: Array<{ type: string; payload: unknown }> = [];
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00.123Z',
			onEvent: (event) => events.push(event),
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser notes' },
		});
		const created = (await server.request('objects_create', {
			input: { type: 'note', title: 'First', body: 'Recover me' },
		})) as { value: WorkspaceObject };

		const deleted = (await server.request('objects_delete', {
			input: { id: created.value.id, expectedRevision: created.value.revision },
		})) as { value: WorkspaceObject; revision: string };
		const trashPath = `.noura/trash/2026-09-12T00-00-00-123Z/${created.value.relativePath}`;

		expect(deleted).toMatchObject({
			value: { id: created.value.id, relativePath: created.value.relativePath },
			revision: created.value.revision,
		});
		expect(values.fileSystem.files.has(created.value.relativePath)).toBe(false);
		expect(values.fileSystem.files.get(trashPath)).toBeDefined();
		const remaining = (await server.request('objects_query', {
			query: {},
		})) as WorkspaceObject[];
		expect(remaining).toHaveLength(0);
		expect(events.at(-1)).toMatchObject({
			type: 'object:deleted',
			payload: {
				id: created.value.id,
				path: created.value.relativePath,
				trashPath,
			},
		});
		const snapshot = (await server.request('workspace_export')) as {
			entries: Array<{ path: string; bytes: Uint8Array }>;
		};
		expect(snapshot.entries.map((entry) => entry.path)).toContain(trashPath);
		const restored = registry();
		const restoredServer = new BrowserWorkspaceServer({
			format,
			registry: restored.registry,
			now: () => '2026-09-12T00:00:00.123Z',
		});
		await restoredServer.request('workspace_import', { snapshot });
		expect(restored.fileSystem.files.get(trashPath)).toEqual(
			values.fileSystem.files.get(trashPath),
		);
	});

	test('rejects a directory whose manifest claims a different workspace identity', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser notes' },
		});
		await server.request('workspace_close');

		await expect(
			server.request('workspace_open', {
				input: { path: 'browser://workspace_01j00000000000000000000001' },
			}),
		).rejects.toMatchObject({ code: 'workspace_identity_mismatch' });
	});

	test('serializes worker requests so a close cannot suppress a committed event', async () => {
		const values = registry();
		const events: string[] = [];
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
			onEvent: (event) => events.push(event.type),
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Browser notes' },
		});

		await Promise.all([
			server.request('objects_create', {
				input: { type: 'note', title: 'First' },
			}),
			server.request('workspace_close'),
		]);

		expect(events).toEqual(['workspace:opened', 'object:created']);
	});

	test('round trips raw workspace bytes and metadata through a new browser workspace', async () => {
		const source = registry();
		const sourceServer = new BrowserWorkspaceServer({
			format,
			registry: source.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await sourceServer.request('workspace_create', {
			input: { path: 'browser://', name: 'Portable notes' },
		});
		const created = (await sourceServer.request('objects_create', {
			input: { type: 'note', title: 'Raw', body: 'Preserve these bytes' },
		})) as { value: WorkspaceObject };
		await source.fileSystem.write(
			'.noura/custom-metadata.bin',
			new Uint8Array([0, 255, 5]),
		);
		const snapshot = await sourceServer.request('workspace_export');

		const destination = registry();
		const destinationServer = new BrowserWorkspaceServer({
			format,
			registry: destination.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await destinationServer.request('workspace_import', { snapshot });

		expect(
			destination.fileSystem.files.get(created.value.relativePath),
		).toEqual(source.fileSystem.files.get(created.value.relativePath));
		expect(
			destination.fileSystem.files.get('.noura/custom-metadata.bin'),
		).toEqual(new Uint8Array([0, 255, 5]));
	});

	test('applies the UI snapshot limits before reading bytes for export', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Large workspace' },
		});
		await values.fileSystem.write(
			'assets/large.bin',
			new Uint8Array(BROWSER_EXPORT_SNAPSHOT_LIMITS.maxEntryBytes + 1),
		);

		await expect(server.request('workspace_export')).rejects.toMatchObject({
			code: 'snapshot_too_large',
		});
	});

	test('allows durable history with an old object ID during snapshot import', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		const workspaceId = 'workspace_01j00000000000000000000000';
		const note = object({
			id: 'note_01j00000000000000000000000',
			title: 'Current note',
			body: '',
			relativePath: 'notes/a.md',
			created: null,
			updated: null,
			properties: {},
		});

		await expect(
			server.request('workspace_import', {
				snapshot: {
					format: 'noura.workspace-snapshot',
					version: 1,
					workspaceId,
					entries: [
						{
							path: '.noura/workspace.yaml',
							bytes: encoder.encode(
								JSON.stringify({
									id: workspaceId,
									name: 'History included',
								}),
							),
						},
						{ path: 'notes/a.md', bytes: encoder.encode(JSON.stringify(note)) },
						{
							path: '.noura/history/note/a.md',
							bytes: encoder.encode(JSON.stringify(note)),
						},
					],
				},
			}),
		).resolves.toMatchObject({ phase: 'ready', workspaceId });
	});

	test('rejects traversal, duplicate identities, and malformed manifests before creating a workspace', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		const workspaceId = 'workspace_01j00000000000000000000000';
		const manifest = encoder.encode(
			JSON.stringify({
				id: workspaceId,
				format_version: 1,
				name: 'Portable notes',
				created: '2026-09-12T00:00:00Z',
				updated: '2026-09-12T00:00:00Z',
				enabled_plugins: [],
				ignore: [],
			}),
		);
		const malformedServer = new BrowserWorkspaceServer({
			format: {
				...format,
				parseWorkspaceManifest: () => {
					throw new WorkspaceFormatError('invalid_manifest');
				},
			},
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await expect(
			malformedServer.request('workspace_import', {
				snapshot: {
					format: 'noura.workspace-snapshot',
					version: 1,
					workspaceId,
					entries: [
						{ path: '.noura/workspace.yaml', bytes: manifest },
						{ path: '../escape.md', bytes: encoder.encode('bad') },
					],
				},
			}),
		).rejects.toMatchObject({ code: 'invalid_workspace_snapshot' });
		const duplicate = object({
			id: 'note_01j00000000000000000000000',
			title: 'Duplicate',
			body: '',
			relativePath: 'notes/a.md',
			created: null,
			updated: null,
			properties: {},
		});
		await expect(
			malformedServer.request('workspace_import', {
				snapshot: {
					format: 'noura.workspace-snapshot',
					version: 1,
					workspaceId,
					entries: [
						{ path: '.noura/workspace.yaml', bytes: manifest },
						{
							path: 'notes/a.md',
							bytes: encoder.encode(JSON.stringify(duplicate)),
						},
						{
							path: 'notes/b.md',
							bytes: encoder.encode(JSON.stringify(duplicate)),
						},
					],
				},
			}),
		).rejects.toMatchObject({ code: 'duplicate_object_identity' });
		await expect(
			malformedServer.request('workspace_import', {
				snapshot: {
					format: 'noura.workspace-snapshot',
					version: 1,
					workspaceId,
					entries: [
						{
							path: '.noura/workspace.yaml',
							bytes: encoder.encode('not json'),
						},
					],
				},
			}),
		).rejects.toMatchObject({ code: 'invalid_manifest' });
		expect(values.fileSystem.files.size).toBe(0);
	});

	test('refuses an import when a stale workspace handle already owns its stable ID', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Existing' },
		});
		const snapshot = await server.request('workspace_export');
		await expect(
			server.request('workspace_import', { snapshot }),
		).rejects.toMatchObject({ code: 'workspace_not_empty' });
		expect(
			((await server.request('manifest_read')) as WorkspaceManifest).name,
		).toBe('Existing');
	});
});

describe('BrowserWorkspaceServer raw file operations', () => {
	async function openServer() {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		await server.request('workspace_create', {
			input: { path: 'browser://', name: 'Raw files' },
		});
		return { server, values };
	}

	test('lists, reads, writes, moves, and deletes canonical bytes', async () => {
		const { server } = await openServer();
		const written = (await server.request('files_write', {
			path: 'assets/blob.bin',
			bytes: 'AAECAw==',
			expectedRevision: null,
		})) as { path: string; revision: string };
		expect(written.path).toBe('assets/blob.bin');

		expect(await server.request('files_list')).toEqual(
			expect.arrayContaining(['assets/blob.bin', '.noura/workspace.yaml']),
		);

		const read = (await server.request('files_read', {
			path: 'assets/blob.bin',
		})) as { revision: string; bytes: string };
		expect(read.revision).toBe(written.revision);
		expect(read.bytes).toBe('AAECAw==');

		const moved = (await server.request('files_move', {
			from: 'assets/blob.bin',
			to: 'assets/renamed.bin',
			expectedRevision: written.revision,
			expectedDestinationRevision: null,
		})) as { path: string; revision: string };
		expect(moved.path).toBe('assets/renamed.bin');
		expect(
			await server.request('files_read', { path: 'assets/blob.bin' }),
		).toBeNull();

		await server.request('files_delete', {
			path: 'assets/renamed.bin',
			expectedRevision: moved.revision,
		});
		expect(
			await server.request('files_read', { path: 'assets/renamed.bin' }),
		).toBeNull();
	});

	test('rejects raw file operations when no workspace is open', async () => {
		const values = registry();
		const server = new BrowserWorkspaceServer({
			format,
			registry: values.registry,
			now: () => '2026-09-12T00:00:00Z',
		});
		const requests: Array<[string, Record<string, unknown>]> = [
			['files_list', {}],
			['files_read', { path: 'notes/a.md' }],
			[
				'files_write',
				{ path: 'notes/a.md', bytes: 'AA==', expectedRevision: null },
			],
			[
				'files_move',
				{
					from: 'notes/a.md',
					to: 'notes/b.md',
					expectedRevision: 'r:x',
					expectedDestinationRevision: null,
				},
			],
			['files_delete', { path: 'notes/a.md', expectedRevision: 'r:x' }],
		];
		for (const [command, payload] of requests) {
			await expect(server.request(command, payload)).rejects.toMatchObject({
				code: 'workspace_not_open',
			});
		}
	});

	test('rejects traversal paths and stale revisions before writing', async () => {
		const { server, values } = await openServer();
		await expect(
			server.request('files_write', {
				path: '../escape.md',
				bytes: 'AA==',
				expectedRevision: null,
			}),
		).rejects.toMatchObject({ code: 'invalid_path' });
		expect(values.fileSystem.files.has('../escape.md')).toBe(false);

		await server.request('files_write', {
			path: 'notes/a.md',
			bytes: 'AA==',
			expectedRevision: null,
		});
		await expect(
			server.request('files_write', {
				path: 'notes/a.md',
				bytes: 'AQ==',
				expectedRevision: null,
			}),
		).rejects.toMatchObject({ code: 'revision_conflict' });
		await expect(
			server.request('files_write', {
				path: 'notes/a.md',
				bytes: 'AQ==',
				expectedRevision: 'r:not-current',
			}),
		).rejects.toMatchObject({ code: 'revision_conflict' });
	});

	test('lists managed objects with their current paths and types', async () => {
		const { server } = await openServer();
		const created = (await server.request('objects_create', {
			input: { type: 'note', title: 'Card', body: 'body' },
		})) as { value: WorkspaceObject };
		const cards = (await server.request('objects_list')) as Array<{
			id: string;
			path: string;
			type: string;
		}>;
		expect(cards).toEqual([
			{
				id: created.value.id,
				path: created.value.relativePath,
				type: 'note',
			},
		]);
	});

	test('rejects malformed raw file payloads', async () => {
		const { server } = await openServer();
		await expect(
			server.request('files_write', {
				path: 'notes/a.md',
				bytes: 'not base64!!',
				expectedRevision: null,
			}),
		).rejects.toMatchObject({ code: 'invalid_base64' });
		await expect(
			server.request('files_write', {
				path: 'notes/a.md',
				bytes: 'AA==',
				expectedRevision: null,
				unexpected: true,
			}),
		).rejects.toMatchObject({ code: 'invalid_input' });
	});
});

test('transport settles concurrent worker responses by request ID', async () => {
	const listeners = new Set<
		(event: MessageEvent<BrowserWorkerResponse>) => void
	>();
	const endpoint: BrowserWorkerEndpoint = {
		postMessage(request: BrowserWorkerRequest) {
			setTimeout(
				() => {
					for (const listener of listeners)
						listener({
							data: {
								type: 'response',
								id: request.id,
								ok: true,
								value: request.command,
							},
						} as MessageEvent<BrowserWorkerResponse>);
				},
				request.command === 'slow' ? 5 : 0,
			);
		},
		addEventListener(type, listener) {
			if (type === 'message')
				listeners.add(
					listener as (event: MessageEvent<BrowserWorkerResponse>) => void,
				);
		},
		removeEventListener(type, listener) {
			if (type === 'message')
				listeners.delete(
					listener as (event: MessageEvent<BrowserWorkerResponse>) => void,
				);
		},
	};
	const transport = createBrowserWorkerTransport(endpoint);
	await expect(
		Promise.all([transport.request('slow'), transport.request('fast')]),
	).resolves.toEqual(['slow', 'fast']);
	transport.dispose();
});
