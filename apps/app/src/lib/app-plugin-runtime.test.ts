import { describe, expect, test } from 'bun:test';
import {
	createNouraClient,
	firstPartyPlugins,
	type CoreTransport,
} from '@noura/workspace';
import type { PluginPlatform } from '@noura/plugin-sdk';
import { AppPluginRuntime } from './app-plugin-runtime';

function harness(platform: PluginPlatform, enabled: string[]) {
	const state = { enabled };
	const calls: string[] = [];
	const transport: CoreTransport = {
		async request<T>(command: string) {
			calls.push(command);
			if (command !== 'manifest_read')
				throw new Error(`Unexpected native call: ${command}`);
			return {
				id: 'workspace_01',
				format_version: 1,
				name: 'Test',
				created: '2026-08-01T00:00:00Z',
				updated: '2026-09-01T00:00:00Z',
				enabled_plugins: state.enabled,
				ignore: [],
			} as T;
		},
		subscribe: async () => () => {},
	};
	return {
		state,
		calls,
		runtime: new AppPluginRuntime(createNouraClient(transport), platform),
	};
}

describe('app plugin runtime', () => {
	for (const platform of ['mobile', 'web'] as const) {
		test(`${platform} skips unsupported plugins without altering durable preferences`, async () => {
			const enabled = [
				...firstPartyPlugins.map((plugin) => plugin.manifest.id),
				'future-plugin',
			];
			const { runtime, calls, state } = harness(platform, enabled);
			expect(runtime.host.platform).toBe(platform);
			expect(await runtime.syncWithManifest()).toEqual({
				activated: [],
				deactivated: [],
				enabledPluginIds: enabled,
			});
			expect(runtime.host.activeManifests()).toEqual([]);
			expect(state.enabled).toEqual(enabled);
			expect(calls).toEqual(['manifest_read']);
		});
	}

	test('desktop activates, reconciles external edits, and cleans up', async () => {
		const { runtime, state } = harness('desktop', [
			'notes',
			'tasks',
			'future-plugin',
		]);
		expect(runtime.host.platform).toBe('desktop');
		expect((await runtime.syncWithManifest()).activated).toEqual([
			'notes',
			'tasks',
		]);
		expect((await runtime.syncWithManifest()).activated).toEqual([]);
		state.enabled = ['notes', 'future-plugin'];
		expect((await runtime.syncWithManifest()).deactivated).toEqual(['tasks']);
		expect(await runtime.deactivateAll()).toEqual(['notes']);
		expect(runtime.host.activeManifests()).toEqual([]);
	});

	test('the entire first-party catalog advertises desktop only', () => {
		for (const plugin of firstPartyPlugins)
			expect(plugin.manifest.platforms).toEqual(['desktop']);
	});
});
