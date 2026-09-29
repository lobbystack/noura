import { describe, expect, test } from 'bun:test';
import type {
	CoreEvent,
	ManagedDraftResult,
	MutationResult,
	RawMarkdownRead,
	WorkspaceEntry,
	WorkspaceObject,
	WorkspaceState,
} from '@noura/shared';
import {
	createServer,
	loadBuiltFormat,
	memoryChannels,
	memoryRegistry,
} from './wasm-test-support';

const format = await loadBuiltFormat();
const encoder = new TextEncoder();
const decoder = new TextDecoder();

async function openWorkspace(
	options: {
		registry?: ReturnType<typeof memoryRegistry>;
		openChannel?: ReturnType<typeof memoryChannels>;
	} = {},
) {
	const values = options.registry ?? memoryRegistry(format!);
	const { server, events } = createServer(format!, values.registry, {
		...(options.openChannel ? { openChannel: options.openChannel } : {}),
	});
	const state = (await server.request('workspace_create', {
		input: { path: 'browser://', name: 'Tests' },
	})) as WorkspaceState;
	const files = values.directories.get(state.workspaceId!)!;
	return { server, events, files, state, values };
}

async function createNote(
	server: { request(command: string, payload?: object): Promise<unknown> },
	title: string,
	relativePath: string,
	body = '',
) {
	const result = (await server.request('objects_create', {
		input: { type: 'note', title, relativePath, body },
	})) as MutationResult<WorkspaceObject>;
	return result.value;
}

function paths(entries: WorkspaceEntry[]) {
	return entries.map((entry) => entry.relativePath);
}

describe.skipIf(format === null)('browser workspace files', () => {
	test('renames a note by path and keeps its stable ID', async () => {
		const { server, events } = await openWorkspace();
		const note = await createNote(server, 'Plan', 'Notes/Plan.md', 'Body');
		events.length = 0;

		await server.request('files_move', {
			input: { from: 'Notes/Plan.md', to: 'Notes/Roadmap.md' },
		});

		const moved = (await server.request('objects_get', {
			id: note.id,
		})) as WorkspaceObject;
		expect(moved.relativePath).toBe('Notes/Roadmap.md');
		expect(events.map((event) => event.type)).toEqual([
			'object:moved',
			'file:changed',
		]);
		expect(events[0]!.payload).toMatchObject({
			id: note.id,
			previousPath: 'Notes/Plan.md',
			path: 'Notes/Roadmap.md',
		});
		await expect(
			server.request('files_move', {
				input: { from: 'Notes/missing.md', to: 'Notes/x.md' },
			}),
		).rejects.toMatchObject({ code: 'file_not_found' });
	});

	test('trashes files and folders into .noura/trash', async () => {
		const { server, files } = await openWorkspace();
		await createNote(server, 'One', 'Folder/One.md');
		await createNote(server, 'Two', 'Folder/Deep/Two.md');
		await createNote(server, 'Three', 'Three.md');

		const trashed = (await server.request('files_trash', {
			input: { relativePath: 'Three.md' },
		})) as string;
		expect(trashed).toMatch(/^\.noura\/trash\/[^/]+\/Three\.md$/);
		expect(files.files.has(trashed)).toBe(true);

		const folder = (await server.request('files_trash', {
			input: { relativePath: 'Folder' },
		})) as string;
		expect(files.files.has(`${folder}/Deep/Two.md`)).toBe(true);
		const entries = (await server.request('files_list')) as WorkspaceEntry[];
		expect(paths(entries)).toEqual([]);
		expect(await server.request('objects_query', {})).toEqual([]);
	});

	test('copies a note as a new object and other files byte for byte', async () => {
		const { server, files } = await openWorkspace();
		const note = await createNote(server, 'Plan', 'Plan.md', 'Body');
		await files.write('image.png', new Uint8Array([1, 2, 3]));
		await server.request('workspace_rebuild_index');

		await server.request('files_copy', {
			input: { from: 'Plan.md', to: 'Plan 1.md' },
		});
		await server.request('files_copy', {
			input: { from: 'image.png', to: 'image 1.png' },
		});
		const objects = (await server.request('objects_query', {
			query: { type: 'note' },
		})) as WorkspaceObject[];
		const copy = objects.find((value) => value.relativePath === 'Plan 1.md')!;
		expect(copy.id).not.toBe(note.id);
		expect(copy.title).toBe('Plan 1');
		expect(copy.body).toBe('Body');
		expect(files.files.get('image 1.png')).toEqual(new Uint8Array([1, 2, 3]));
		await expect(
			server.request('files_copy', {
				input: { from: 'Plan.md', to: 'image.png' },
			}),
		).rejects.toMatchObject({ code: 'path_exists' });
	});

	test('creates, lists, moves and removes folders', async () => {
		const { server } = await openWorkspace();
		await server.request('folders_create', {
			input: { relativePath: 'Empty' },
		});
		await createNote(server, 'Inside', 'Box/Inside.md');
		expect(
			paths((await server.request('files_list')) as WorkspaceEntry[]),
		).toEqual(['Box', 'Box/Inside.md', 'Empty']);

		await server.request('folders_move', {
			input: { from: 'Box', to: 'Empty/Box' },
		});
		expect(
			paths((await server.request('files_list')) as WorkspaceEntry[]),
		).toEqual(['Empty', 'Empty/Box', 'Empty/Box/Inside.md']);
		await expect(
			server.request('folders_remove', { input: { relativePath: 'Empty' } }),
		).rejects.toMatchObject({ code: 'folder_not_empty' });
		await server.request('folders_create', {
			input: { relativePath: 'Gone' },
		});
		await server.request('folders_remove', { input: { relativePath: 'Gone' } });
		expect(await server.request('folders_list')).toEqual([
			{ relativePath: 'Empty', name: 'Empty' },
			{ relativePath: 'Empty/Box', name: 'Box' },
		]);
	});
});

describe.skipIf(format === null)('browser raw Markdown', () => {
	test('keeps line endings and merges a change made elsewhere', async () => {
		const { server, files } = await openWorkspace();
		await files.write(
			'Loose.md',
			encoder.encode('# Loose\r\n\r\nfirst\r\nmiddle\r\nsecond\r\n'),
		);
		await server.request('workspace_rebuild_index');

		const read = (await server.request('raw_markdown_read', {
			relativePath: 'Loose.md',
		})) as RawMarkdownRead;
		expect(read.usesCrlf).toBe(true);
		expect(read.body).toBe('# Loose\n\nfirst\nmiddle\nsecond\n');

		// Another tab edits the last line while this one edits the first.
		await files.write(
			'Loose.md',
			encoder.encode('# Loose\r\n\r\nfirst\r\nmiddle\r\nsecond, changed\r\n'),
		);
		await server.request('workspace_rebuild_index');
		const saved = (await server.request('raw_markdown_save', {
			input: {
				relativePath: 'Loose.md',
				baseRevision: read.revision,
				baseBody: read.body,
				localBody: '# Loose\n\nfirst, edited\nmiddle\nsecond\n',
			},
		})) as { status: string; current: RawMarkdownRead };
		expect(saved.status).toBe('saved');
		expect(decoder.decode(files.files.get('Loose.md'))).toBe(
			'# Loose\r\n\r\nfirst, edited\r\nmiddle\r\nsecond, changed\r\n',
		);
		expect(
			[...files.files.keys()].some((path) =>
				path.startsWith('.noura/history/raw-'),
			),
		).toBe(true);

		const conflict = (await server.request('raw_markdown_save', {
			input: {
				relativePath: 'Loose.md',
				baseRevision: read.revision,
				baseBody: read.body,
				localBody: '# Loose\n\nfirst\nmiddle\nsecond, mine\n',
			},
		})) as { status: string };
		expect(conflict.status).toBe('conflict');
	});
});

describe.skipIf(format === null)('browser drafts', () => {
	test('saves, merges and flags managed drafts like native', async () => {
		const { server } = await openWorkspace();
		const created = (await server.request('objects_create', {
			input: {
				type: 'task',
				title: 'Ship',
				relativePath: 'Tasks/Ship.md',
				body: 'one\ntwo\n',
			},
		})) as MutationResult<WorkspaceObject>;
		const base = created.value;
		const draft = {
			id: base.id,
			baseRevision: base.revision,
			baseTitle: base.title,
			baseBody: base.body,
			baseProperties: base.properties,
		};

		const saved = (await server.request('managed_draft_save', {
			input: {
				...draft,
				localTitle: 'Ship it',
				localBody: base.body,
				localProperties: { ...base.properties, priority: 'high' },
			},
		})) as ManagedDraftResult;
		expect(saved.status).toBe('unchanged');
		expect(saved.current).toMatchObject({
			title: 'Ship it',
			properties: { priority: 'high' },
		});

		const merged = (await server.request('managed_draft_save', {
			input: {
				...draft,
				localTitle: base.title,
				localBody: 'one\ntwo\nthree\n',
				localProperties: base.properties,
			},
		})) as ManagedDraftResult;
		expect(merged.status).toBe('merged');
		expect(merged.current).toMatchObject({
			title: 'Ship it',
			body: 'one\ntwo\nthree',
			properties: { priority: 'high' },
		});

		const conflict = (await server.request('managed_draft_save', {
			input: {
				...draft,
				localTitle: base.title,
				localBody: base.body,
				localProperties: { ...base.properties, priority: 'low' },
			},
		})) as ManagedDraftResult;
		expect(conflict.status).toBe('conflict');
	});

	test('reconciles a note body without writing', async () => {
		const { server } = await openWorkspace();
		const note = await createNote(server, 'Plan', 'Plan.md', 'a\nb\n');
		const unchanged = (await server.request('notes_reconcile_draft', {
			input: {
				id: note.id,
				baseRevision: note.revision,
				baseBody: note.body,
				localBody: 'a\nb\nc\n',
			},
		})) as { status: string; body: string };
		expect(unchanged).toMatchObject({ status: 'unchanged', body: 'a\nb\nc\n' });
	});
});

describe.skipIf(format === null)('browser duplicates and other tabs', () => {
	test('a duplicated ID blocks only operations addressed to it', async () => {
		const { server, files } = await openWorkspace();
		const note = await createNote(server, 'Original', 'A.md');
		const other = await createNote(server, 'Other', 'B.md');
		files.files.set('Copy.md', files.files.get('A.md')!.slice());
		await server.request('workspace_rebuild_index');

		await expect(
			server.request('objects_update', {
				id: note.id,
				patch: { expectedRevision: note.revision, title: 'Changed' },
			}),
		).rejects.toMatchObject({ code: 'identity_conflict' });
		const updated = (await server.request('objects_update', {
			id: other.id,
			patch: { expectedRevision: other.revision, title: 'Still works' },
		})) as MutationResult<WorkspaceObject>;
		expect(updated.value.title).toBe('Still works');
		expect(
			((await server.request('objects_query', {})) as WorkspaceObject[]).map(
				(value) => value.id,
			),
		).toEqual([other.id]);
		expect(decoder.decode(files.files.get('Copy.md'))).toBe(
			decoder.decode(files.files.get('A.md')),
		);
	});

	test('another tab’s change reaches this tab as external events', async () => {
		const registry = memoryRegistry(format!);
		const openChannel = memoryChannels();
		const first = await openWorkspace({ registry, openChannel });
		const second = createServer(format!, registry.registry, { openChannel });
		await second.server.request('workspace_open', {
			input: { path: `browser://${first.state.workspaceId}` },
		});
		expect(await second.server.request('objects_query', {})).toEqual([]);
		second.events.length = 0;

		const note = await createNote(first.server, 'Shared', 'Shared.md');
		await second.server.request('workspace_state');

		const received = second.events.filter(
			(event: CoreEvent) => event.source === 'external',
		);
		expect(received.map((event) => event.type)).toEqual([
			'object:created',
			'file:changed',
		]);
		expect(
			(
				(await second.server.request('objects_query', {})) as WorkspaceObject[]
			)[0]?.id,
		).toBe(note.id);
	});

	test('search finds words by prefix in titles and bodies', async () => {
		const { server } = await openWorkspace();
		const note = await createNote(
			server,
			'Release plan',
			'Plan.md',
			'Ship the build',
		);
		await createNote(server, 'Groceries', 'Food.md', 'Milk');
		const results = (await server.request('search_query', {
			input: { query: 'rele bui', type: null, pathPrefix: null, limit: null },
		})) as Array<{ objectId: string }>;
		expect(results.map((result) => result.objectId)).toEqual([note.id]);
	});
});
