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

	async sync() {
		try {
			const result = await getPluginRuntime().syncWithManifest();
			const runtime = getPluginRuntime();
			this.activeManifests = [...runtime.host.activeManifests()];
			this.enabledIds = result.enabledPluginIds;
			this.lastError = null;
		} catch (error) {
			if (isCoreError(error) && error.code === 'workspace_not_open') {
				// Onboarding or a closed workspace: no plugins are scoped to
				// any manifest, so the feature surfaces hide.
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
	['notes', '/notes'],
	['tasks', '/tasks'],
	['calendar', '/calendar'],
	['projects', '/projects'],
];
