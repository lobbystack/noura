import { PluginHost, type PluginHostServices } from '@noura/plugin-sdk';
import { firstPartyPlugins } from './first-party';
import type { NouraClient } from './index';

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
	/** Verbatim from .noura/workspace.yaml; may include unknown future plugin ids. */
	enabledPluginIds: Array<string>;
}

/**
 * Keeps the set of active plugins aligned with `enabled_plugins` in
 * .noura/workspace.yaml. Unknown manifest ids are tolerated and ignored so
 * ecosystem plugins cannot break older builds, and a durable file
 * edit drives the runtime: the manifest on disk is authoritative.
 */
export class PluginRuntime {
	readonly host: PluginHost;
	#client: NouraClient;

	constructor(client: NouraClient, host?: PluginHost) {
		this.#client = client;
		this.host = host ?? new PluginHost(createPluginHostServices(client));
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
		for (const plugin of firstPartyPlugins) {
			if (
				enabled.has(plugin.manifest.id) &&
				!this.host.isActive(plugin.manifest.id)
			) {
				await this.host.activate(plugin);
				activated.push(plugin.manifest.id);
			}
		}
		return {
			activated,
			deactivated,
			enabledPluginIds: manifest.enabledPlugins,
		};
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
