import { describe, expect, test } from 'bun:test';
import {
	AiRegistry,
	browserPluginCapabilities,
	CommandRegistry,
	createNouraClient,
	type CoreTransport,
} from './index';
import { PluginRuntime, createPluginHostServices } from './plugin-runtime';
import type { PluginContext, PluginDefinition } from '@noura/plugin-sdk';

function harness(initialEnabled: Array<string>) {
	const store = new Map<string, unknown>();
	const state = {
		enabled: initialEnabled,
		updated: '2026-09-01T00:00:00Z',
	};
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
					updated: state.updated,
					enabled_plugins: state.enabled,
					ignore: [],
				} as T;
			}
			if (command === 'plugin_state_get') {
				const { pluginId, key } = payload as Record<string, string>;
				const value = store.get(`${pluginId}:${key}`);
				return (value === undefined ? null : value) as T;
			}
			if (command === 'manifest_update') {
				const input = payload?.input as {
					enabledPlugins: string[] | null;
					expectedUpdated: string | null;
				};
				if (
					input.expectedUpdated !== null &&
					input.expectedUpdated !== state.updated
				)
					throw {
						code: 'manifest_conflict',
						category: 'conflict',
						operation: 'manifest_update',
					};
				if (input.enabledPlugins !== null) state.enabled = input.enabledPlugins;
				state.updated = '2026-09-01T00:00:01Z';
				return {
					id: 'workspace_01',
					format_version: 1,
					name: 'Test',
					created: '2026-08-01T00:00:00Z',
					updated: state.updated,
					enabled_plugins: state.enabled,
					ignore: [],
				} as T;
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
	const aiRegistry = new AiRegistry();
	const client = createNouraClient(transport, undefined, aiRegistry);
	return {
		aiRegistry,
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

	test('web activates the supported note, task, project, and calendar contracts without AI', async () => {
		const { aiRegistry, client } = harness([
			'notes',
			'tasks',
			'projects',
			'ai',
			'folders',
			'calendar',
		]);
		const runtime = new PluginRuntime(client, {
			platform: 'web',
			supportedCapabilities: browserPluginCapabilities,
		});
		const result = await runtime.syncWithManifest();
		expect(result.activated).toEqual([
			'notes',
			'tasks',
			'calendar',
			'projects',
		]);
		expect(result.unavailablePluginIds).toEqual(['ai', 'folders']);
		expect(
			runtime.host.activeManifests().map((manifest) => manifest.id),
		).toEqual(['notes', 'tasks', 'calendar', 'projects']);
		expect(await client.commands.list()).toEqual([
			{ id: 'notes.create', title: 'Create note' },
			{ id: 'tasks.create', title: 'Create task' },
			{ id: 'tasks.complete', title: 'Complete task' },
			{ id: 'projects.create', title: 'Create project' },
		]);
		expect(aiRegistry.toolEntries()).toEqual([]);
		// The calendar plugin is active on web but contributes no AI context.
		expect(aiRegistry.contextProviders()).toEqual([]);
	});

	test('the web runtime rejects a calendar AI-capability attempt', async () => {
		const { client } = harness(['calendar']);
		const runtime = new PluginRuntime(client, {
			platform: 'web',
			supportedCapabilities: browserPluginCapabilities,
		});
		await expect(
			runtime.host.activate({
				manifest: {
					id: 'calendar',
					name: 'Calendar',
					version: '0.1.0',
					capabilities: [
						'workspace.objects',
						'workspace.events',
						'ai.context',
						'ai.tools',
					],
					platforms: ['desktop', 'web'],
					activationCapabilities: {
						web: ['workspace.objects', 'workspace.events'],
					},
				},
				activate(context) {
					context.ai.registerContextProvider({
						id: 'calendar.upcoming-week',
						provide: async () => [],
					});
				},
			}),
		).rejects.toThrow('cannot use unavailable ai.context');
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

	test('persists a disabled plugin preference and deactivates it from the manifest', async () => {
		const { runtime, state } = harness(['notes', 'tasks']);
		await runtime.syncWithManifest();
		const preference = await runtime.registry.read();
		const result = await runtime.setEnabled('tasks', false, preference.updated);
		expect(state.enabled).toEqual(['notes']);
		expect(result.deactivated).toEqual(['tasks']);
		expect(result.enabledPluginIds).toEqual(['notes']);
		expect(runtime.host.isActive('tasks')).toBe(false);
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
		const { aiRegistry, client } = harness([]);
		const services = createPluginHostServices(client);
		expect(services.objects).toBe(client.objects);
		expect(services.search).toBe(client.search);
		expect(services.events).toBe(client.events);
		expect(services.commands).toBe(client.commands);
		expect(services.storage).toBe(client.pluginState);
		services.ai.registerInstructionProvider(
			{
				id: 'adapter.instructions',
				provide: async () => 'Use workspace data.',
			},
			{ owner: 'adapter' },
		);
		expect(aiRegistry.instructionEntries()[0]).toMatchObject({
			owner: 'adapter',
			category: 'instructions',
		});
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
				if (command === 'objects_query' || command === 'files_list')
					return [] as T;
				return null as T;
			},
			subscribe: async () => () => {},
		};
		const aiRegistry = new AiRegistry();
		const client = createNouraClient(transport, undefined, aiRegistry);
		const runtime = new PluginRuntime(client);
		await runtime.syncWithManifest();
		return { aiRegistry, client, calls, runtime, store };
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
			'projects.create',
		]);
	});

	test('enabled domains register public JSON-schema AI tools', async () => {
		const { aiRegistry } = await activatedClient([
			'notes',
			'tasks',
			'calendar',
			'projects',
			'folders',
		]);
		const tools = aiRegistry.tools();
		expect(tools.map((tool) => tool.name)).toEqual([
			'calendar.upcoming',
			'folders.list',
			'notes.create',
			'projects.create',
			'tasks.complete',
			'tasks.create',
		]);
		expect(
			tools.find((tool) => tool.name === 'notes.create')?.inputSchema,
		).toMatchObject({
			type: 'object',
			required: ['title'],
			properties: { title: { type: 'string' }, body: { type: 'string' } },
		});
		expect(
			tools.find((tool) => tool.name === 'tasks.complete')?.inputSchema,
		).toMatchObject({
			type: 'object',
			required: ['id', 'expectedRevision'],
			properties: {
				id: { type: 'string' },
				expectedRevision: { type: 'string' },
			},
		});
		expect(
			tools.find((tool) => tool.name === 'projects.create')?.inputSchema,
		).toMatchObject({
			type: 'object',
			required: ['title'],
			properties: {
				title: { type: 'string' },
				body: { type: 'string' },
				properties: { type: 'object' },
			},
		});
		expect(
			tools.find((tool) => tool.name === 'calendar.upcoming')?.inputSchema,
		).toEqual({
			type: 'object',
			properties: { days: { type: 'integer', minimum: 1, maximum: 31 } },
			additionalProperties: false,
		});
		expect(
			tools.find((tool) => tool.name === 'folders.list')?.inputSchema,
		).toEqual({ type: 'object', additionalProperties: false });
	});

	test('AI tools reuse typed object services and preserve expected revisions', async () => {
		const { aiRegistry, calls } = await activatedClient([
			'notes',
			'tasks',
			'projects',
			'calendar',
			'folders',
		]);
		const tool = (name: string) =>
			aiRegistry.tools().find((value) => value.name === name)!;

		await tool('notes.create').execute({
			title: 'AI note',
			body: '# AI note',
		});
		expect(calls.at(-1)).toEqual({
			command: 'objects_create',
			payload: {
				input: { type: 'note', title: 'AI note', body: '# AI note' },
			},
		});
		await tool('tasks.create').execute({ title: 'AI task' });
		expect(calls.at(-1)).toEqual({
			command: 'objects_create',
			payload: { input: { type: 'task', title: 'AI task' } },
		});
		await tool('projects.create').execute({
			title: 'AI project',
			properties: {},
		});
		expect(calls.at(-1)).toEqual({
			command: 'objects_create',
			payload: {
				input: {
					type: 'project',
					title: 'AI project',
					body: undefined,
					properties: {},
				},
			},
		});
		expect(await tool('calendar.upcoming').execute({ days: 7 })).toEqual([]);
		expect(await tool('folders.list').execute({})).toEqual([]);
		await expect(
			tool('tasks.complete').execute({ id: 'task_01' }),
		).rejects.toThrow('expected revision');
		await tool('tasks.complete').execute({
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
						{
							id: 'task_settled',
							type: 'task',
							title: 'Already done',
							properties: { status: 'done', due: tomorrow() },
						},
						{
							id: 'project_settled',
							type: 'project',
							title: 'Cancelled project',
							properties: { status: 'cancelled', due: tomorrow() },
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
		const context = await providers[0]!.provide({
			workspaceId: 'workspace_01',
		});
		// Upcoming context is outstanding work: the done task and the
		// cancelled project above are settled and must not surface.
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
