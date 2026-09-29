import { expect, test } from 'bun:test';
import {
	definePlugin,
	PluginHost,
	PluginRuntimeError,
	requireCapability,
	supportsPlatform,
	type CollaborationProvider,
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

test('platform metadata gates activation while legacy manifests remain portable', async () => {
	let desktopActivated = false;
	const desktopOnly = definePlugin({
		manifest: {
			id: 'desktop-only',
			name: 'Desktop only',
			version: '1.0.0',
			capabilities: [],
			platforms: ['desktop'],
		},
		activate() {
			desktopActivated = true;
		},
	});
	expect(supportsPlatform(desktopOnly.manifest, 'desktop')).toBe(true);
	expect(supportsPlatform(desktopOnly.manifest, 'web')).toBe(false);
	expect(() =>
		definePlugin({
			manifest: {
				id: 'no-platforms',
				name: 'No platforms',
				version: '1.0.0',
				capabilities: [],
				platforms: [],
			},
			activate() {},
		}),
	).toThrow();

	const webHost = new PluginHost({} as PluginHostServices, {
		platform: 'web',
	});
	await expect(webHost.activate(desktopOnly)).rejects.toThrow(
		'does not support web',
	);
	expect(desktopActivated).toBe(false);
	expect(webHost.isActive('desktop-only')).toBe(false);

	let activated = false;
	await webHost.activate(
		definePlugin({
			manifest: {
				id: 'legacy',
				name: 'Legacy',
				version: '1.0.0',
				capabilities: [],
			},
			activate() {
				activated = true;
			},
		}),
	);
	expect(activated).toBe(true);
});

test('partial hosts reject unavailable activation capabilities before plugin code runs', async () => {
	let activated = false;
	const host = new PluginHost({} as PluginHostServices, {
		platform: 'web',
		supportedCapabilities: ['workspace.objects'],
	});
	await expect(
		host.activate(
			definePlugin({
				manifest: {
					id: 'requires-commands',
					name: 'Requires commands',
					version: '1.0.0',
					capabilities: ['workspace.objects', 'workspace.commands'],
					platforms: ['web'],
					activationCapabilities: {
						web: ['workspace.objects', 'workspace.commands'],
					},
				},
				activate() {
					activated = true;
				},
			}),
		),
	).rejects.toMatchObject({
		code: 'plugin_capability_unsupported',
		category: 'validation',
		operation: 'plugin_activate',
	});
	expect(activated).toBe(false);
	const error = new PluginRuntimeError(
		'plugin_capability_unsupported',
		'Unavailable',
		'plugin_activate',
		{},
	);
	expect(error.retryable).toBe(false);
});

test('a context reports only the capabilities the activation actually holds', async () => {
	const seen: {
		capabilities?: ReadonlySet<string>;
		held?: Array<boolean>;
	} = {};
	const full = new PluginHost({} as PluginHostServices);
	await full.activate({
		manifest: {
			id: 'grants',
			name: 'Grants',
			version: '1.0.0',
			capabilities: ['workspace.objects', 'ai.context'],
		},
		activate(context) {
			seen.capabilities = context.capabilities;
			seen.held = [
				context.hasCapability('workspace.objects'),
				context.hasCapability('ai.context'),
				context.hasCapability('workspace.files'),
			];
		},
	});
	expect([...seen.capabilities!]).toEqual(['workspace.objects', 'ai.context']);
	expect(seen.held).toEqual([true, true, false]);

	const partial = new PluginHost({} as PluginHostServices, {
		platform: 'web',
		supportedCapabilities: ['workspace.objects'],
	});
	await partial.activate({
		manifest: {
			id: 'partial-grants',
			name: 'Partial grants',
			version: '1.0.0',
			capabilities: ['workspace.objects', 'ai.context'],
			platforms: ['web'],
			activationCapabilities: { web: ['workspace.objects'] },
		},
		activate(context) {
			seen.capabilities = context.capabilities;
			seen.held = [
				context.hasCapability('workspace.objects'),
				context.hasCapability('ai.context'),
			];
		},
	});
	expect([...seen.capabilities!]).toEqual(['workspace.objects']);
	expect(seen.held).toEqual([true, false]);
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
		collaboration: {
			service: collaborationService(),
			registerProvider: () => () => true,
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
		collaboration: {
			service: collaborationService(),
			registerProvider: () => () => true,
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
		collaboration: {
			service: collaborationService(),
			registerProvider: () => () => true,
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

test('plugin manifests are frozen snapshots that cannot be expanded after validation', () => {
	const manifest = {
		id: 'frozen',
		name: 'Frozen',
		version: '1.0.0',
		capabilities: ['workspace.search'] as (
			'workspace.search' | 'workspace.storage'
		)[],
	};
	const plugin = definePlugin({
		manifest,
		activate() {},
	});
	// Mutating the caller's original object must not change the snapshot.
	manifest.capabilities.push('workspace.storage');
	manifest.id = 'frozen-renamed';
	expect(() =>
		requireCapability(plugin.manifest, 'workspace.storage'),
	).toThrow();
	expect(plugin.manifest.id).toBe('frozen');
	expect(Object.isFrozen(plugin.manifest)).toBe(true);
	expect(Object.isFrozen(plugin.manifest.capabilities)).toBe(true);
});

test('plugin host snapshots a raw definition manifest at activation', async () => {
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
			registerInstructionProvider: () => () => true,
		},
		collaboration: {
			service: collaborationService(),
			registerProvider: () => () => true,
		},
	} satisfies PluginHostServices);
	const raw = {
		manifest: {
			id: 'raw',
			name: 'Raw',
			version: '1.0.0',
			capabilities: [] as ('workspace.storage' | 'workspace.search')[],
		},
		activate() {},
	};
	await host.activate(raw);
	raw.manifest.capabilities.push('workspace.storage');
	raw.manifest.id = 'raw-renamed';
	expect(host.activeManifests()).toEqual([
		{
			id: 'raw',
			name: 'Raw',
			version: '1.0.0',
			capabilities: [],
		},
	]);
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
		collaboration: {
			service: collaborationService(),
			registerProvider: () => () => true,
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

function collaborationService(): CollaborationProvider {
	return {
		open: async () => null,
		submitUpdates: async () => ({ revision: 'rev' }) as never,
		flush: async () => {},
		close: async () => {},
		setPresence: async () => {},
	};
}

function collaborationHost(
	trustedPlugins: Iterable<ReturnType<typeof definePlugin>>,
	log: string[],
) {
	let provider: CollaborationProvider | null = null;
	const service = collaborationService();
	const host = new PluginHost(
		{
			collaboration: {
				service,
				registerProvider: (
					value: CollaborationProvider,
					registration: { owner: string },
				) => {
					log.push(`register:${registration.owner}`);
					provider = value;
					return () => {
						log.push(`dispose:${registration.owner}`);
						const had = provider === value;
						if (had) provider = null;
						return had;
					};
				},
			},
		} as unknown as PluginHostServices,
		{ trustedPlugins },
	);
	return { host, service, provider: () => provider };
}

function syncLikePlugin(log: string[]) {
	return definePlugin({
		manifest: {
			id: 'sync',
			name: 'Sync',
			version: '0.1.0',
			capabilities: ['workspace.collaboration'],
		},
		activate(context) {
			log.push('activate');
			context.collaboration.registerProvider(context.collaboration.service);
		},
		async deactivate() {
			log.push('deactivate');
			await Promise.resolve();
		},
	});
}

test('an untrusted plugin cannot hold the collaboration capability', async () => {
	const log: string[] = [];
	const plugin = syncLikePlugin(log);
	const { host, provider } = collaborationHost([], log);
	expect(host.activationError(plugin)).toMatchObject({
		code: 'plugin_capability_untrusted',
		operation: 'plugin_activate',
		details: { pluginId: 'sync', capability: 'workspace.collaboration' },
	});
	await expect(host.activate(plugin)).rejects.toMatchObject({
		code: 'plugin_capability_untrusted',
	});
	// A copy of a trusted definition is a different plugin.
	const trusted = collaborationHost([plugin], log);
	await expect(
		trusted.host.activate({ ...plugin, activate: plugin.activate }),
	).rejects.toMatchObject({ code: 'plugin_capability_untrusted' });
	expect(log).toEqual([]);
	expect(provider()).toBeNull();
});

test('a trusted plugin registers the collaboration provider', async () => {
	const log: string[] = [];
	const plugin = syncLikePlugin(log);
	const { host, provider } = collaborationHost([plugin], log);
	expect(host.activationError(plugin)).toBeNull();
	await host.activate(plugin);
	expect(log).toEqual(['activate', 'register:sync']);
	expect(provider()).not.toBeNull();
	expect(await provider()!.open({ relativePath: 'note.md' })).toBeNull();
});

test('the collaboration provider is disposed before an async deactivate runs', async () => {
	const log: string[] = [];
	const plugin = syncLikePlugin(log);
	const { host, provider } = collaborationHost([plugin], log);
	await host.activate(plugin);
	const deactivation = host.deactivate('sync');
	expect(provider()).toBeNull();
	expect(log).toEqual([
		'activate',
		'register:sync',
		'dispose:sync',
		'deactivate',
	]);
	expect(await deactivation).toBe(true);
});
