import { describe, expect, test } from 'bun:test';
import {
	AiRegistry,
	CommandRegistry,
	createNouraClient,
	type CoreTransport,
} from './index';
import { PluginRuntime, createPluginHostServices } from './plugin-runtime';
import type { PluginContext, PluginDefinition } from '@noura/plugin-sdk';

function harness(initialEnabled: Array<string>) {
	const store = new Map<string, unknown>();
	const state = { enabled: initialEnabled };
	const cleanupCalls: Array<string> = [];
	const calls: Array<{ command: string; payload?: Record<string, unknown> }> =
		[];
	const transport: CoreTransport = {
		request: async <T>(command: string, payload?: Record<string, unknown>) => {
			calls.push(payload === undefined ? { command } : { command, payload });
			if (command === 'manifest_read') {
				return {
					id: 'workspace_01',
					format_version: 1,
					name: 'Test',
					created: '2026-08-01T00:00:00Z',
					updated: '2026-09-01T00:00:00Z',
					enabled_plugins: state.enabled,
					ignore: [],
				} as T;
			}
			if (command === 'plugin_state_get') {
				const { pluginId, key } = payload as Record<string, string>;
				const value = store.get(`${pluginId}:${key}`);
				return (value === undefined ? null : value) as T;
			}
			if (command === 'plugin_state_set') {
				const { pluginId, key, value } = payload as Record<string, string>;
				store.set(`${pluginId}:${key}`, value);
				return null as T;
			}
			if (command === 'plugin_state_delete') {
				const { pluginId, key } = payload as Record<string, string>;
				return store.delete(`${pluginId}:${key}`) as unknown as T;
			}
			return null as T;
		},
		subscribe: async () => () => {},
	};
	const client = createNouraClient(transport);
	return {
		client,
		calls,
		store,
		state,
		cleanupCalls,
		runtime: new PluginRuntime(client),
	};
}

function storagePlugin(
	id: string,
	cleanupCalls: Array<string>,
): PluginDefinition {
	const contextRef: { current?: PluginContext } = {};
	return {
		manifest: {
			id,
			name: id,
			version: '0.1.0',
			capabilities: ['workspace.storage'],
		},
		activate(api) {
			contextRef.current = api;
			return api.storage.set('view', { layout: 'board' }).then(() => {});
		},
		async deactivate(api) {
			if (api !== contextRef.current) {
				throw new Error('deactivate must receive the activation context');
			}
			await api.storage.delete('view');
			cleanupCalls.push(id);
		},
	};
}

describe('plugin runtime', () => {
	test('activates exactly the first-party plugins listed in the manifest', async () => {
		const { runtime } = harness(['notes', 'tasks']);
		const result = await runtime.syncWithManifest();
		expect(result.activated).toEqual(['notes', 'tasks']);
		expect(result.deactivated).toEqual([]);
		expect(
			runtime.host.activeManifests().map((manifest) => manifest.id),
		).toEqual(['notes', 'tasks']);
	});

	test('tolerates unknown plugin ids from future ecosystem plugins', async () => {
		const { runtime } = harness(['notes', 'crm-future']);
		const result = await runtime.syncWithManifest();
		expect(
			runtime.host.activeManifests().map((manifest) => manifest.id),
		).toEqual(['notes']);
		expect(result.enabledPluginIds).toEqual(['notes', 'crm-future']);
	});

	test('deactivates and re-activates live as the file manifest changes', async () => {
		const { runtime, state } = harness(['notes', 'tasks']);
		await runtime.syncWithManifest();
		state.enabled = ['notes'];
		const shrank = await runtime.syncWithManifest();
		expect(shrank.deactivated).toEqual(['tasks']);
		expect(
			runtime.host.activeManifests().map((manifest) => manifest.id),
		).toEqual(['notes']);
		state.enabled = ['notes', 'tasks', 'projects'];
		const grew = await runtime.syncWithManifest();
		expect(grew.activated).toEqual(['tasks', 'projects']);
		expect(
			runtime.host.activeManifests().map((manifest) => manifest.id),
		).toEqual(['notes', 'tasks', 'projects']);
	});

	test('plugin storage is namespaced by plugin id and deactivation cleans up', async () => {
		const { runtime, calls, store, cleanupCalls } = harness([]);
		await runtime.syncWithManifest();
		await runtime.host.activate(storagePlugin('pasteboard', cleanupCalls));
		expect([...store.keys()]).toEqual(['pasteboard:view']);
		const storageCall = calls.find(
			(call) => call.command === 'plugin_state_set',
		);
		expect(storageCall?.payload).toEqual({
			pluginId: 'pasteboard',
			key: 'view',
			value: { layout: 'board' },
		});
		await runtime.host.deactivate('pasteboard');
		expect(runtime.host.isActive('pasteboard')).toBe(false);
		expect(store.size).toBe(0);
		expect(cleanupCalls).toEqual(['pasteboard']);
	});

	test('capability guard fires before storage reaches the services', async () => {
		const { runtime } = harness([]);
		await expect(
			runtime.host.activate({
				manifest: {
					id: 'sneaky',
					name: 'Sneaky',
					version: '0.1.0',
					capabilities: [],
				},
				activate(api) {
					return api.storage.set('view', 'nope') as never;
				},
			}),
		).rejects.toThrow('does not declare workspace.storage');
	});

	test('host services pass through the typed client contracts', async () => {
		const { client } = harness([]);
		const services = createPluginHostServices(client);
		expect(services.objects).toBe(client.objects);
		expect(services.search).toBe(client.search);
		expect(services.events).toBe(client.events);
		expect(services.commands).toBe(client.commands);
		expect(services.storage).toBe(client.pluginState);
	});
});

describe('first-party plugin dogfood', () => {
	async function activatedClient(enabled: Array<string>) {
		const store = new Map<string, unknown>();
		const calls: Array<{ command: string; payload?: Record<string, unknown> }> =
			[];
		const transport: CoreTransport = {
			request: async <T>(
				command: string,
				payload?: Record<string, unknown>,
			) => {
				calls.push(payload === undefined ? { command } : { command, payload });
				if (command === 'manifest_read') {
					return {
						id: 'workspace_01',
						format_version: 1,
						name: 'Test',
						created: '2026-08-01T00:00:00Z',
						updated: '2026-09-01T00:00:00Z',
						enabled_plugins: enabled,
						ignore: [],
					} as T;
				}
				if (command === 'plugin_state_get') return null as T;
				if (command === 'objects_create') {
					return {
						value: { id: 'task-durable' },
						revision: 'rev-next',
						durability: 'committed',
						indexStatus: 'updated',
						warnings: [],
					} as T;
				}
				if (command === 'objects_update') {
					return {
						value: { id: 'task_01', properties: { status: 'done' } },
						revision: 'rev-done',
						durability: 'committed',
						indexStatus: 'updated',
						warnings: [],
					} as T;
				}
				return null as T;
			},
			subscribe: async () => () => {},
		};
		const client = createNouraClient(transport);
		const runtime = new PluginRuntime(client);
		await runtime.syncWithManifest();
		return { client, calls, runtime, store };
	}

	test('enabled domains register their commands through the capability surface', async () => {
		const { client } = await activatedClient([
			'notes',
			'tasks',
			'calendar',
			'projects',
			'folders',
		]);
		const commands = await client.commands.list();
		expect(commands.map((command) => command.id)).toEqual([
			'notes.create',
			'tasks.create',
			'tasks.complete',
		]);
	});

	test('tasks.create runs as a guarded durable object create', async () => {
		const { client, calls } = await activatedClient(['tasks']);
		const created = await client.commands.execute<{ id: string }>(
			'tasks.create',
			{ title: 'Ship plugin runtime' },
		);
		expect(created).toStrictEqual({ id: 'task-durable' });
		expect(calls.at(-1)).toEqual({
			command: 'objects_create',
			payload: {
				input: { type: 'task', title: 'Ship plugin runtime' },
			},
		});
		expect(client.commands.execute('tasks.create', {})).rejects.toThrow(
			'A task title is required',
		);
	});

	test('tasks.complete requires an expected revision', async () => {
		const { client, calls } = await activatedClient(['tasks']);
		await expect(
			client.commands.execute('tasks.complete', { id: 'task_01' }),
		).rejects.toThrow('expected revision');
		await client.commands.execute('tasks.complete', {
			id: 'task_01',
			expectedRevision: 'rev-1',
		});
		expect(calls.at(-1)).toEqual({
			command: 'objects_update',
			payload: {
				id: 'task_01',
				patch: { expectedRevision: 'rev-1', properties: { status: 'done' } },
			},
		});
	});

	test('notes.create writes a note object', async () => {
		const { client, calls } = await activatedClient(['notes']);
		await client.commands.execute('notes.create', {
			title: 'Meeting notes',
			body: '# Meeting notes',
		});
		expect(calls.at(-1)).toEqual({
			command: 'objects_create',
			payload: {
				input: {
					type: 'note',
					title: 'Meeting notes',
					body: '# Meeting notes',
				},
			},
		});
		expect(
			client.commands.execute('notes.create', { title: '   ' }),
		).rejects.toThrow('title is required');
	});

	test('disabling a domain removes its commands live', async () => {
		const store = new Map<string, unknown>();
		const calls: Array<{ command: string; payload?: Record<string, unknown> }> =
			[];
		let enabled = ['notes', 'tasks'];
		const transport: CoreTransport = {
			request: async <T>(
				command: string,
				payload?: Record<string, unknown>,
			) => {
				calls.push(payload === undefined ? { command } : { command, payload });
				if (command === 'manifest_read') {
					return {
						id: 'workspace_01',
						format_version: 1,
						name: 'Test',
						created: '2026-08-01T00:00:00Z',
						updated: '2026-09-01T00:00:00Z',
						enabled_plugins: enabled,
						ignore: [],
					} as T;
				}
				if (command === 'plugin_state_get') return null as T;
				return null as T;
			},
			subscribe: async () => () => {},
		};
		const client = createNouraClient(transport);
		const runtime = new PluginRuntime(client);
		await runtime.syncWithManifest();
		expect((await client.commands.list()).map((c) => c.id)).toEqual([
			'notes.create',
			'tasks.create',
			'tasks.complete',
		]);
		enabled = ['notes'];
		await runtime.syncWithManifest();
		expect((await client.commands.list()).map((c) => c.id)).toEqual([
			'notes.create',
		]);
	});

	test('the calendar plugin registers an AI context provider over public capabilities', async () => {
		const aiRegistry = new AiRegistry();
		const store = new Map<string, unknown>();
		const transport: CoreTransport = {
			request: async <T>(
				command: string,
				payload?: Record<string, unknown>,
			) => {
				if (command === 'manifest_read') {
					return {
						id: 'workspace_01',
						format_version: 1,
						name: 'Test',
						created: '2026-08-01T00:00:00Z',
						updated: '2026-09-01T00:00:00Z',
						enabled_plugins: ['calendar'],
						ignore: [],
					} as T;
				}
				if (command === 'objects_query') {
					return [
						{
							id: 'task_due_soon',
							type: 'task',
							title: 'Due soon',
							properties: { due: tomorrow() },
						},
						{
							id: 'task_no_date',
							type: 'task',
							title: 'No date',
							properties: { status: 'todo' },
						},
					] as T;
				}
				if (command === 'plugin_state_get') return null as T;
				void payload;
				void store;
				return null as T;
			},
			subscribe: async () => () => {},
		};
		const client = createNouraClient(
			transport,
			new CommandRegistry(),
			aiRegistry,
		);
		const runtime = new PluginRuntime(client);
		await runtime.syncWithManifest();
		const providers = aiRegistry.contextProviders();
		expect(providers.map((provider) => provider.id)).toEqual([
			'calendar.upcoming-week',
		]);
		const context = await providers[0].provide({ workspaceId: 'workspace_01' });
		expect(context).toEqual([
			{
				title: 'Due soon',
				content: `${tomorrow()} (task)`,
				sourceId: 'task_due_soon',
			},
		]);
	});
});

function tomorrow() {
	const date = new Date(Date.now() + 24 * 60 * 60 * 1000);
	return date.toISOString().slice(0, 10);
}
