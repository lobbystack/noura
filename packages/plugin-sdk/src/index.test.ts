import { expect, test } from 'bun:test';
import {
	definePlugin,
	PluginHost,
	requireCapability,
	type PluginHostServices,
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
	const services: PluginHostServices = {
		files: {
			list: async () => [],
			listNonManagedMarkdown: async () => [],
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
		storage: {
			get: async () => undefined,
			set: async () => {},
			delete: async () => false,
		},
		ai: {
			registerTool: () => () => true,
			registerContextProvider: () => () => true,
		},
	} satisfies PluginHostServices;
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

test('plugin host guards non-managed Markdown discovery', async () => {
	const services: PluginHostServices = {
		files: {
			list: async () => [],
			listNonManagedMarkdown: async () => [],
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
		storage: {
			get: async () => undefined,
			set: async () => {},
			delete: async () => false,
		},
		ai: {
			registerTool: () => () => true,
			registerContextProvider: () => () => true,
		},
	} satisfies PluginHostServices;
	const host = new PluginHost(services);

	await expect(
		host.activate(
			definePlugin({
				manifest: {
					id: 'limited-files',
					name: 'Limited files',
					version: '1.0.0',
					capabilities: [],
				},
				activate(context) {
					return context.files.listNonManagedMarkdown().then(() => {});
				},
			}),
		),
	).rejects.toThrow('does not declare workspace.files');
});

test('plugin host deactivation runs the cleanup and updates active state', async () => {
	let disposed = false;
	const host = new PluginHost({
		files: {
			list: async () => [],
			listNonManagedMarkdown: async () => [],
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
		storage: {
			get: async () => undefined,
			set: async () => {},
			delete: async () => false,
		},
		ai: {
			registerTool: () => () => true,
			registerContextProvider: () => () => true,
		},
	} satisfies PluginHostServices);
	await host.activate(
		definePlugin({
			manifest: {
				id: 'disposable',
				name: 'Disposable',
				version: '1.0.0',
				capabilities: [],
			},
			activate() {},
			deactivate() {
				disposed = true;
			},
		}),
	);
	expect(host.isActive('disposable')).toBe(true);
	expect(await host.deactivate('disposable')).toBe(true);
	expect(disposed).toBe(true);
	expect(host.isActive('disposable')).toBe(false);
	expect(await host.deactivate('disposable')).toBe(false);
});
