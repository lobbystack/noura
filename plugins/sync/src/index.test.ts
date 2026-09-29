import { describe, expect, test } from 'bun:test';
import {
	PluginHost,
	type CollaborationProvider,
	type PluginHostServices,
} from '@noura/plugin-sdk';
import sync from './index';

function host(platform: 'desktop' | 'web', trusted: boolean) {
	const registered: CollaborationProvider[] = [];
	const service: CollaborationProvider = {
		open: async () => null,
		submitUpdates: async () => {
			throw new Error('not used');
		},
		flush: async () => {},
		close: async () => {},
		setPresence: async () => {},
	};
	const services = {
		collaboration: {
			service,
			registerProvider: (provider: CollaborationProvider) => {
				registered.push(provider);
				return () =>
					registered.splice(registered.indexOf(provider), 1).length > 0;
			},
		},
	} as unknown as PluginHostServices;
	return {
		registered,
		host: new PluginHost(services, {
			platform,
			trustedPlugins: trusted ? [sync] : [],
			...(platform === 'web'
				? { supportedCapabilities: ['workspace.objects' as const] }
				: {}),
		}),
	};
}

describe('sync plugin', () => {
	test('declares its id, platforms and trusted capability', () => {
		expect(sync.manifest).toEqual({
			id: 'sync',
			name: 'Sync',
			version: '0.1.0',
			capabilities: ['workspace.collaboration'],
			platforms: ['desktop', 'mobile', 'web'],
			activationCapabilities: { web: [] },
		});
	});

	test('registers the native collaboration provider on desktop', async () => {
		const { host: desktop, registered } = host('desktop', true);
		await desktop.activate(sync);
		expect(registered).toHaveLength(1);
		await desktop.deactivate('sync');
		expect(registered).toHaveLength(0);
	});

	test('activates on the web without a provider', async () => {
		const { host: web, registered } = host('web', true);
		await web.activate(sync);
		expect(web.isActive('sync')).toBe(true);
		expect(registered).toHaveLength(0);
	});

	test('needs the host to trust it', async () => {
		const { host: desktop } = host('desktop', false);
		await expect(desktop.activate(sync)).rejects.toMatchObject({
			code: 'plugin_capability_untrusted',
		});
	});
});
