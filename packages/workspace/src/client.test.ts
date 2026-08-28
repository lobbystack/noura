import { describe, expect, test } from 'bun:test';
import {
	createNouraClient,
	firstPartyPlugins,
	type CoreTransport,
	type Task,
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
