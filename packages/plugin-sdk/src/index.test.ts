import { expect, test } from 'bun:test';
import {
	definePlugin,
	PluginHost,
	requireCapability,
	type PluginContext,
} from './index';

test('plugin manifests enforce declared capabilities', () => {
	const plugin = definePlugin({
		manifest: {
			id: 'example',
			name: 'Example',
			version: '1.0.0',
			capabilities: ['workspace.search'],
		},
		activate() {},
	});
	expect(() =>
		requireCapability(plugin.manifest, 'workspace.storage'),
	).toThrow();
});
test('plugin host denies undeclared object access', async () => {
	const services = {
		files: {
			list: async () => [],
			createFolder: async () => {},
			moveFolder: async () => {},
			removeEmptyFolder: async () => {},
		},
		objects: {
			list: async () => [],
			get: async () => {
				throw new Error();
			},
			create: async () => {
				throw new Error();
			},
			update: async () => {
				throw new Error();
			},
		},
		search: { query: async () => [] },
		events: { subscribe: async () => () => {} },
		commands: { register: () => () => {} },
		storage: { get: async () => undefined, set: async () => {} },
		ai: {
			registerTool: () => () => true,
			registerContextProvider: () => () => true,
		},
	} as PluginContext;
	const host = new PluginHost(services);
	await expect(
		host.activate(
			definePlugin({
				manifest: {
					id: 'limited',
					name: 'Limited',
					version: '1.0.0',
					capabilities: ['workspace.search'],
				},
				activate(context) {
					return context.objects.list().then(() => {});
				},
			}),
		),
	).rejects.toThrow('does not declare workspace.objects');
});
