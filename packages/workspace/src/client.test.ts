import { describe, expect, test } from 'bun:test';
import {
	createNouraClient,
	firstPartyPlugins,
	type CoreTransport,
	type Task,
	type UnmanagedFile,
	type WorkspaceEntry,
} from './index';

function transport(responses: Record<string, unknown>): CoreTransport {
	const calls: Array<{ command: string; payload?: Record<string, unknown> }> =
		[];
	return {
		calls,
		request: async <T>(command: string, payload?: Record<string, unknown>) => {
			calls.push(payload === undefined ? { command } : { command, payload });
			return responses[command] as T;
		},
		subscribe: async () => () => {},
	} as CoreTransport & { calls: typeof calls };
}

describe('typed client', () => {
	test('registers every Initial MVP domain as a first-party plugin', () => {
		expect(firstPartyPlugins.map((plugin) => plugin.manifest.id)).toEqual([
			'folders',
			'notes',
			'tasks',
			'calendar',
			'projects',
		]);
	});
	test('delegates native workspace folder selection through the typed boundary', async () => {
		const mock = transport({ workspace_pick_folder: '/Users/example/Notes' });
		const client = createNouraClient(mock);

		await expect(
			client.workspaces.pickFolder({ title: 'Open a Noura workspace' }),
		).resolves.toBe('/Users/example/Notes');
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> })
				.calls[0],
		).toEqual({
			command: 'workspace_pick_folder',
			payload: { title: 'Open a Noura workspace' },
		});
	});
	test('delegates workspace file discovery through the typed boundary', async () => {
		const entries: WorkspaceEntry[] = [
			{
				relativePath: 'notes',
				name: 'notes',
				kind: 'folder',
				parseStatus: null,
				objectId: null,
				objectType: null,
				revision: null,
			},
		];
		const mock = transport({ files_list: entries });
		const client = createNouraClient(mock);

		await expect(client.files.list()).resolves.toEqual(entries);
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> })
				.calls[0],
		).toEqual({ command: 'files_list' });
	});
	test('delegates non-managed Markdown discovery through the typed boundary', async () => {
		const files: UnmanagedFile[] = [
			{
				relativePath: 'draft.md',
				title: 'Draft',
				body: '',
				revision: 'abc',
				parseStatus: 'unmanaged',
				parseError: null,
			},
		];
		const mock = transport({ files_list_non_managed_markdown: files });
		const client = createNouraClient(mock);

		await expect(client.files.listNonManagedMarkdown()).resolves.toEqual(files);
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> })
				.calls[0],
		).toEqual({ command: 'files_list_non_managed_markdown' });
	});
	test('task completion delegates to generic revision-checked object update', async () => {
		const mock = transport({
			objects_update: {
				value: {},
				revision: 'next',
				durability: 'committed',
				indexStatus: 'updated',
				warnings: [],
			},
		});
		const client = createNouraClient(mock);
		await client.tasks.complete({ id: 'task_01k', expectedRevision: 'old' });
		expect(
			(
				mock as CoreTransport & {
					calls: Array<{ command: string; payload?: Record<string, unknown> }>;
				}
			).calls[0],
		).toEqual({
			command: 'objects_update',
			payload: {
				id: 'task_01k',
				patch: { expectedRevision: 'old', properties: { status: 'done' } },
			},
		});
	});
	test('delegates draft reconciliation and explicit resolution through typed note commands', async () => {
		const mock = transport({
			notes_reconcile_draft: { status: 'conflict', current: {}, body: null },
			notes_resolve_conflict: { value: {}, revision: 'next' },
		});
		const client = createNouraClient(mock);
		await client.notes.reconcileDraft({
			id: 'note_01k',
			baseRevision: 'base',
			baseBody: 'before',
			localBody: 'local',
		});
		await client.notes.resolveConflict({
			id: 'note_01k',
			currentRevision: 'current',
			localBody: 'local',
			resolution: 'replace-external',
		});
		expect(
			(mock as CoreTransport & { calls: Array<Record<string, unknown>> }).calls,
		).toEqual([
			{
				command: 'notes_reconcile_draft',
				payload: {
					input: {
						id: 'note_01k',
						baseRevision: 'base',
						baseBody: 'before',
						localBody: 'local',
					},
				},
			},
			{
				command: 'notes_resolve_conflict',
				payload: {
					input: {
						id: 'note_01k',
						currentRevision: 'current',
						localBody: 'local',
						resolution: 'replace-external',
					},
				},
			},
		]);
	});
	test('kanban projection groups and orders indexed tasks without board storage', async () => {
		const tasks = [
			{
				id: 'task_b',
				type: 'task',
				title: 'B',
				body: '',
				relativePath: 'b.md',
				revision: '1',
				properties: { status: 'todo', priority: 'medium', kanban_order: 'b' },
			},
			{
				id: 'task_a',
				type: 'task',
				title: 'A',
				body: '',
				relativePath: 'a.md',
				revision: '1',
				properties: { status: 'todo', priority: 'medium', kanban_order: 'a' },
			},
		] as Task[];
		const client = createNouraClient(transport({ objects_query: tasks }));
		const board = await client.kanban.getBoard();
		expect(board.groups[0]?.items.map((task) => task.id)).toEqual([
			'task_a',
			'task_b',
		]);
	});
});
