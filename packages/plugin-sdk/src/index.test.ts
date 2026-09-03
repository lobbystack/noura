import { expect, test } from 'bun:test';
import {
	definePlugin,
	PluginHost,
	requireCapability,
	type PluginHostServices,
} from './index';
import { AiRegistry } from '@noura/ai';

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
			registerInstructionProvider: () => () => true,
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
			registerInstructionProvider: () => () => true,
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
	let commandDisposed = 0;
	let eventDisposed = 0;
	let releaseDeactivate!: () => void;
	const deactivationPaused = new Promise<void>((resolve) => {
		releaseDeactivate = resolve;
	});
	let deactivateStarted!: () => void;
	const deactivationStarted = new Promise<void>((resolve) => {
		deactivateStarted = resolve;
	});
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
		events: {
			subscribe: async () => () => {
				eventDisposed += 1;
			},
		},
		commands: {
			register: () => () => {
				commandDisposed += 1;
			},
		},
		storage: {
			get: async () => undefined,
			set: async () => {},
			delete: async () => false,
		},
		ai: {
			registerTool: () => () => true,
			registerContextProvider: () => () => true,
			registerInstructionProvider: () => () => true,
		},
	} satisfies PluginHostServices);
	await host.activate(
		definePlugin({
			manifest: {
				id: 'disposable',
				name: 'Disposable',
				version: '1.0.0',
				capabilities: ['workspace.commands', 'workspace.events'],
			},
			async activate(context) {
				context.commands.register({
					id: 'disposable.command',
					title: 'Disposable command',
					execute: async () => {},
				});
				await context.events.subscribe(() => {});
			},
			async deactivate() {
				disposed = true;
				deactivateStarted();
				await deactivationPaused;
			},
		}),
	);
	expect(host.isActive('disposable')).toBe(true);
	const deactivation = host.deactivate('disposable');
	await deactivationStarted;
	expect(disposed).toBe(true);
	expect(commandDisposed).toBe(1);
	expect(eventDisposed).toBe(1);
	expect(host.isActive('disposable')).toBe(false);
	releaseDeactivate();
	expect(await deactivation).toBe(true);
	expect(await host.deactivate('disposable')).toBe(false);
});

test('AI instruction registrations are capability-gated, owned, and removed on disable', async () => {
	const registry = new AiRegistry();
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
			registerTool: (definition, registration) =>
				registry.registerTool(definition, registration),
			registerContextProvider: (definition, registration) =>
				registry.registerContextProvider(definition, registration),
			registerInstructionProvider: (definition, registration) =>
				registry.registerInstructionProvider(definition, registration),
		},
	} satisfies PluginHostServices);
	await host.activate({
		manifest: {
			id: 'instructions',
			name: 'Instructions',
			version: '1.0.0',
			capabilities: ['ai.instructions'],
		},
		activate(context) {
			context.ai.registerInstructionProvider({
				id: 'instructions.style',
				provide: async () => 'Use short sentences.',
			});
		},
	});
	expect(registry.instructionEntries()[0]).toMatchObject({
		owner: 'instructions',
		category: 'instructions',
	});
	await host.deactivate('instructions');
	expect(registry.instructionEntries()).toEqual([]);
});
