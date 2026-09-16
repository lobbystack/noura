import {
	PluginHost,
	type PluginCapability,
	type PluginDefinition,
	type PluginHostOptions,
	type PluginHostServices,
} from '@noura/plugin-sdk';
import { firstPartyPlugins } from './first-party';
import type { NouraClient } from './client';

/**
 * The browser worker implements only these plugin-facing services. Notes,
 * tasks, projects, and the read-only calendar view activate with a subset of
 * these; `ai.*` and `workspace.files` are never available in the browser.
 */
export const browserPluginCapabilities: readonly PluginCapability[] = [
	'workspace.objects',
	'workspace.commands',
	'workspace.events',
];

/**
 * Adapter from the typed workspace client to the capability-gated
 * plugin services. Pass-through only: plugins see the same public
 * contracts the UI consumes, never transport internals.
 */
export function createPluginHostServices(
	client: NouraClient,
): PluginHostServices {
	return {
		files: {
			list: () => client.files.list(),
			listNonManagedMarkdown: () => client.files.listNonManagedMarkdown(),
			createFolder: (relativePath) => client.folders.create({ relativePath }),
			moveFolder: (from, to) => client.folders.move({ from, to }),
			removeEmptyFolder: (relativePath) =>
				client.folders.removeEmpty({ relativePath }),
		},
		objects: client.objects,
		search: client.search,
		events: client.events,
		commands: client.commands,
		storage: client.pluginState,
		ai: {
			registerTool: (definition, registration) =>
				client.ai.registerTool(definition, registration),
			registerContextProvider: (definition, registration) =>
				client.ai.registerContextProvider(definition, registration),
			registerInstructionProvider: (definition, registration) =>
				client.ai.registerInstructionProvider(definition, registration),
		},
	};
}

export interface PluginSyncResult {
	activated: Array<string>;
	deactivated: Array<string>;
	/** Enabled manifests this host intentionally cannot activate. */
	unavailablePluginIds: Array<string>;
	/** Verbatim from .noura/workspace.yaml; may include unknown future plugin ids. */
	enabledPluginIds: Array<string>;
}

export interface PluginRegistrySnapshot {
	enabledPluginIds: Array<string>;
	updated: string;
}

/**
 * Durable plugin preferences backed by `.noura/workspace.yaml`. The client
 * transport owns canonical validation and serialization; this registry never
 * writes manifest bytes or keeps a separate preference store.
 */
export class PluginRegistry {
	constructor(private readonly client: Pick<NouraClient, 'manifest'>) {}

	async read(): Promise<PluginRegistrySnapshot> {
		const manifest = await this.client.manifest.read();
		return {
			enabledPluginIds: manifest.enabledPlugins,
			updated: manifest.updated,
		};
	}

	async setEnabled(
		pluginId: string,
		enabled: boolean,
		expectedUpdated: string,
	): Promise<PluginRegistrySnapshot> {
		const current = await this.client.manifest.read();
		const enabledPluginIds = new Set(current.enabledPlugins);
		if (enabled) enabledPluginIds.add(pluginId);
		else enabledPluginIds.delete(pluginId);
		const manifest = await this.client.manifest.update({
			enabledPlugins: [...enabledPluginIds],
			expectedUpdated,
		});
		return {
			enabledPluginIds: manifest.enabledPlugins,
			updated: manifest.updated,
		};
	}
}

export interface PluginRuntimeOptions extends PluginHostOptions {
	plugins?: readonly PluginDefinition[];
}

/**
 * Keeps the set of active plugins aligned with `enabled_plugins` in
 * .noura/workspace.yaml. Unknown manifest ids are tolerated and ignored so
 * ecosystem plugins cannot break older builds, and a durable file
 * edit drives the runtime: the manifest on disk is authoritative.
 */
export class PluginRuntime {
	readonly host: PluginHost;
	readonly registry: PluginRegistry;
	#client: NouraClient;
	#plugins: readonly PluginDefinition[];

	constructor(
		client: NouraClient,
		optionsOrHost: PluginRuntimeOptions | PluginHost = {},
		host?: PluginHost,
	) {
		const options = optionsOrHost instanceof PluginHost ? {} : optionsOrHost;
		const configuredHost =
			optionsOrHost instanceof PluginHost ? optionsOrHost : host;
		this.#client = client;
		this.registry = new PluginRegistry(client);
		this.#plugins = options.plugins ?? firstPartyPlugins;
		this.host =
			configuredHost ??
			new PluginHost(createPluginHostServices(client), options);
	}

	async syncWithManifest(): Promise<PluginSyncResult> {
		const manifest = await this.#client.manifest.read();
		const enabled = new Set(manifest.enabledPlugins);
		const deactivated: Array<string> = [];
		for (const active of this.host.activeManifests()) {
			if (!enabled.has(active.id)) {
				if (await this.host.deactivate(active.id)) deactivated.push(active.id);
			}
		}
		const activated: Array<string> = [];
		const unavailablePluginIds: Array<string> = [];
		for (const plugin of this.#plugins) {
			if (
				enabled.has(plugin.manifest.id) &&
				!this.host.isActive(plugin.manifest.id)
			) {
				const error = this.host.activationError(plugin);
				if (
					error?.code === 'plugin_platform_unsupported' ||
					error?.code === 'plugin_capability_unsupported'
				) {
					unavailablePluginIds.push(plugin.manifest.id);
					continue;
				}
				if (error) throw error;
				await this.host.activate(plugin);
				activated.push(plugin.manifest.id);
			}
		}
		return {
			activated,
			deactivated,
			unavailablePluginIds,
			enabledPluginIds: manifest.enabledPlugins,
		};
	}

	/** Persists a preference first, then brings this host to the durable state. */
	async setEnabled(
		pluginId: string,
		enabled: boolean,
		expectedUpdated: string,
	): Promise<PluginSyncResult & PluginRegistrySnapshot> {
		const preference = await this.registry.setEnabled(
			pluginId,
			enabled,
			expectedUpdated,
		);
		return { ...(await this.syncWithManifest()), ...preference };
	}

	/**
	 * Deactivate every active plugin. Runs when no workspace is scoped
	 * anymore (close, onboarding): commands, AI contributions, and event
	 * subscriptions must not linger in the singleton runtime until the next
	 * workspace opens and reconciles.
	 */
	async deactivateAll(): Promise<Array<string>> {
		const deactivated: Array<string> = [];
		for (const active of this.host.activeManifests()) {
			if (await this.host.deactivate(active.id)) deactivated.push(active.id);
		}
		return deactivated;
	}
}
