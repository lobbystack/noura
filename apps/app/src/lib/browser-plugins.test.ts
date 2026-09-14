import { expect, test } from 'bun:test';
import {
	createNouraClient,
	PluginRuntime,
	browserPluginCapabilities,
	firstPartyPlugins,
	type CoreTransport,
} from '@noura/workspace/browser';
import { createBrowserPluginModel } from './browser-plugins';

function harness(failure: 'none' | 'activation' | 'rollback' = 'none') {
	let enabled = ['notes'];
	let revision = 1;
	let ready = true;
	let subscriptions = 0;
	const transport: CoreTransport = {
		async request<T>(command: string, payload?: unknown): Promise<T> {
			if (command === 'workspace_state')
				return { phase: ready ? 'ready' : 'closed' } as T;
			if (command === 'manifest_update') {
				const { input } = payload as {
					input: { expectedUpdated: string; enabledPlugins: string[] };
				};
				if (input.expectedUpdated !== String(revision))
					throw {
						code: 'manifest_conflict',
						message: 'Stale manifest revision',
					};
				enabled = input.enabledPlugins;
				revision++;
			} else if (command !== 'manifest_read')
				throw new Error(`Unexpected command ${command}`);
			return {
				id: 'workspace',
				name: 'Test',
				format_version: 1,
				created: '1',
				updated: String(revision),
				enabled_plugins: enabled,
				ignore: [],
			} as T;
		},
		async subscribe() {
			subscriptions++;
			return () => {
				subscriptions--;
			};
		},
	};
	const client = createNouraClient(transport);
	const runtime = new PluginRuntime(client, {
		platform: 'web',
		supportedCapabilities: browserPluginCapabilities,
		plugins: firstPartyPlugins.map((plugin) =>
			plugin.manifest.id !== 'tasks' || failure === 'none'
				? plugin
				: {
						...plugin,
						activate() {
							if (failure === 'rollback') revision++;
							throw new Error('Activation failed');
						},
					},
		),
	});
	const model = createBrowserPluginModel(client, runtime);
	return {
		model,
		runtime,
		get enabled() {
			return enabled;
		},
		get subscriptions() {
			return subscriptions;
		},
		external() {
			enabled = ['notes', 'calendar'];
			revision++;
		},
		close() {
			ready = false;
		},
	};
}

test('active projection, scoped cleanup, unsupported preferences and disposal', async () => {
	const h = harness();
	await h.model.init();
	expect(h.model.snapshot().isEnabled('notes')).toBe(true);
	expect(h.model.snapshot().isSupported('calendar')).toBe(false);
	await h.model.snapshot().setEnabled('tasks', true);
	expect(h.model.snapshot().isEnabled('tasks')).toBe(true);
	h.external();
	await h.model.sync();
	expect(h.model.snapshot().enabledIds).toContain('calendar');
	expect(h.model.snapshot().isEnabled('calendar')).toBe(false);
	expect(h.model.snapshot().isEnabled('tasks')).toBe(false);
	await h.model.scope(async () => {
		expect(h.runtime.host.activeManifests()).toEqual([]);
		h.close();
	});
	expect(h.model.snapshot().isEnabled('notes')).toBe(false);
	await h.model.dispose();
	expect(h.subscriptions).toBe(0);
});

test('stale displayed revision rejects without overwriting another tab', async () => {
	const h = harness();
	await h.model.init();
	h.external();
	await expect(h.model.snapshot().setEnabled('tasks', true)).rejects.toThrow(
		'Nothing was overwritten',
	);
	expect(h.enabled).toEqual(['notes', 'calendar']);
	expect(h.model.snapshot().enabledIds).toEqual(h.enabled);
	await h.model.dispose();
});

test('activation failure restores the preference with CAS and reports it', async () => {
	const h = harness('activation');
	await h.model.init();
	await expect(h.model.snapshot().setEnabled('tasks', true)).rejects.toThrow(
		'previous preference was restored',
	);
	expect(h.enabled).toEqual(['notes']);
	expect(h.model.snapshot().isEnabled('tasks')).toBe(false);
	await h.model.dispose();
});

test('failed rollback is not presented as success and runtime access fails closed', async () => {
	const h = harness('rollback');
	await h.model.init();
	await expect(h.model.snapshot().setEnabled('tasks', true)).rejects.toThrow(
		'Rollback could not be confirmed',
	);
	expect(h.enabled).toContain('tasks');
	expect(h.model.snapshot().lastError).toContain('reconciliation failed');
	expect(h.model.snapshot().isEnabled('notes')).toBe(false);
	await h.model.dispose();
});
