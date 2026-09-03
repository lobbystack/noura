import { browser } from '$app/environment';
import { isCoreError, type PluginManifest } from '@noura/workspace';
import { getNouraClient, getPluginRuntime } from './state.svelte';

function errorMessage(error: unknown) {
	if (error instanceof Error) return error.message;
	if (error && typeof error === 'object' && 'message' in error) {
		return String(error.message);
	}
	return String(error);
}

/**
 * Keeps the UI reflecting workspace.yaml: which first-party plugins are
 * enabled, which the runtime currently has active, and how to toggle them.
 * The manifest file is authoritative; this store only projects it.
 */
class PluginStore {
	activeManifests = $state<PluginManifest[]>([]);
	enabledIds = $state<Array<string>>([]);
	lastError = $state<string | null>(null);
	/** True once the first sync for a workspace context has settled. */
	synced = $state(false);

	#initialized = false;
	#syncChain: Promise<void> = Promise.resolve();
	#unlisten: (() => void) | undefined;

	get activeIds(): Array<string> {
		return this.enabledIds.filter((id) =>
			this.activeManifests.some((manifest) => manifest.id === id),
		);
	}

	isEnabled(id: string): boolean {
		return (
			this.enabledIds.includes(id) &&
			this.activeManifests.some((manifest) => manifest.id === id)
		);
	}

	async init() {
		if (!browser || this.#initialized) return;
		this.#initialized = true;
		await this.sync();
		try {
			this.#unlisten = await getNouraClient().events.subscribe((event) => {
				if (
					event.type === 'workspace:ready' ||
					event.type === 'workspace:manifest-updated' ||
					event.type === 'workspace:closed' ||
					event.type === 'file:changed'
				) {
					void this.sync();
				}
			});
		} catch {
			this.#initialized = false;
		}
	}

	/**
	 * Synchronize with the manifest on disk. Runs are serialized: event
	 * bursts and the explicit sync after `setEnabled` chain onto one
	 * in-flight run, so two overlapping reconciliations can never both
	 * pass the activation checks and double-activate a plugin (whose
	 * lifecycle may await, per the plugin-sdk contract).
	 */
	async sync() {
		const run = this.#syncChain.then(() => this.#runSync());
		// Keep the chain resolvable even if a run ever rejects.
		this.#syncChain = run.then(undefined, () => undefined);
		await run;
	}

	async #runSync() {
		try {
			const result = await getPluginRuntime().syncWithManifest();
			const runtime = getPluginRuntime();
			this.activeManifests = [...runtime.host.activeManifests()];
			this.enabledIds = result.enabledPluginIds;
			this.lastError = null;
		} catch (error) {
			if (isCoreError(error) && error.code === 'workspace_not_open') {
				// Onboarding or a closed workspace: no manifest is scoped,
				// so the feature surfaces hide — and the runtime must let go
				// of every plugin so its commands and AI contributions do
				// not linger in the singleton registries.
				await getPluginRuntime().deactivateAll();
				this.activeManifests = [];
				this.enabledIds = [];
				this.lastError = null;
				return;
			}
			this.lastError = errorMessage(error);
		} finally {
			this.synced = true;
		}
	}

	async setEnabled(pluginId: string, enabled: boolean) {
		const client = getNouraClient();
		const manifest = await client.manifest.read();
		const next = enabled
			? [...new Set([...manifest.enabledPlugins, pluginId])].sort()
			: manifest.enabledPlugins.filter((id) => id !== pluginId);
		await client.manifest.update({
			enabledPlugins: next,
			expectedUpdated: manifest.updated,
		});
		await this.sync();
	}
}

export const plugins = new PluginStore();

/** Routes backed by a first-party plugin, mapped to the plugin's id. */
export const PLUGIN_ROUTES: ReadonlyArray<readonly [string, string]> = [
	['ai', '/ai'],
	['notes', '/notes'],
	['tasks', '/tasks'],
	['calendar', '/calendar'],
	['projects', '/projects'],
];
