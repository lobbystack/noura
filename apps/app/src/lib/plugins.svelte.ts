import { browser } from '$app/environment';
import {
	firstPartyPlugins,
	isCoreError,
	type PluginManifest,
} from '@noura/workspace';
import { supportsPlatform } from '@noura/plugin-sdk';
import { getAppPlatform, platformLabels } from './platform';
import {
	movePlugin,
	normalizePluginOrder,
	SIDEBAR_PLUGIN_IDS,
	type NavigationId,
} from './plugin-order';
import { getNouraClient, getPluginRuntime, workspace } from './state.svelte';

const PLUGIN_ORDER_STORAGE_PREFIX = 'noura.plugin-order.v1:';

function errorMessage(error: unknown) {
	if (error instanceof Error) return error.message;
	if (error && typeof error === 'object' && 'message' in error) {
		return String(error.message);
	}
	return String(error);
}

/**
 * Keeps the UI reflecting .noura/workspace.yaml: which first-party plugins are
 * enabled, which the runtime currently has active, and how to toggle them.
 * The manifest file is authoritative; this store only projects it.
 */
class PluginStore {
	get platform() {
		return getAppPlatform();
	}
	get catalog() {
		return firstPartyPlugins.map((plugin) => plugin.manifest);
	}

	isSupported(id: string): boolean {
		const manifest = this.catalog.find((plugin) => plugin.id === id);
		return (
			!!manifest &&
			this.platform !== null &&
			supportsPlatform(manifest, this.platform)
		);
	}

	unavailableReason(id: string): string | null {
		if (!this.platform)
			return 'Native platform could not be detected. Start or build through the Tauri CLI.';
		if (!this.isSupported(id))
			return `Not available on ${platformLabels[this.platform]}.`;
		if (!workspace.isReady) return 'Open a workspace to manage this plugin.';
		if (!this.synced) return 'Loading workspace plugin preferences…';
		if (this.lastError)
			return 'Plugin preferences could not be loaded. Try again.';
		return null;
	}
	activeManifests = $state<PluginManifest[]>([]);
	enabledIds = $state<Array<string>>([]);
	pluginOrder = $state<NavigationId[]>(normalizePluginOrder());
	lastError = $state<string | null>(null);
	/** True once the first sync for a workspace context has settled. */
	synced = $state(false);

	#initialized = false;
	#syncChain: Promise<void> = Promise.resolve();
	#unlisten: (() => void) | undefined;
	#orderWorkspaceId: string | null = null;

	get activeIds(): Array<string> {
		return this.enabledIds.filter((id) => this.isEnabled(id));
	}

	get orderedPluginIds(): NavigationId[] {
		return this.pluginOrder;
	}

	get orderedSidebarPluginIds(): NavigationId[] {
		return this.pluginOrder.filter((id) =>
			(SIDEBAR_PLUGIN_IDS as readonly string[]).includes(id),
		);
	}

	isEnabled(id: string): boolean {
		return (
			this.isSupported(id) &&
			this.enabledIds.includes(id) &&
			this.activeManifests.some((manifest) => manifest.id === id)
		);
	}

	async init() {
		if (!browser || this.#initialized) return;
		if (this.platform === 'web' || !this.platform) {
			this.synced = true;
			return;
		}
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
		if (this.platform === 'web' || !this.platform) {
			this.activeManifests = [];
			this.enabledIds = [];
			this.synced = true;
			return;
		}
		try {
			const result = await getPluginRuntime().syncWithManifest();
			const runtime = getPluginRuntime();
			this.activeManifests = [...runtime.host.activeManifests()];
			this.enabledIds = result.enabledPluginIds;
			this.#loadPluginOrder(workspace.state?.workspaceId ?? null);
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
				this.#loadPluginOrder(null);
				this.lastError = null;
				return;
			}
			this.lastError = errorMessage(error);
		} finally {
			this.synced = true;
		}
	}

	move(pluginId: string, targetPluginId: string, after: boolean) {
		const next = movePlugin(this.pluginOrder, pluginId, targetPluginId, after);
		if (next.every((id, index) => id === this.pluginOrder[index])) return;
		this.pluginOrder = next;
		if (browser && this.#orderWorkspaceId) {
			try {
				localStorage.setItem(
					`${PLUGIN_ORDER_STORAGE_PREFIX}${this.#orderWorkspaceId}`,
					JSON.stringify(next),
				);
			} catch {
				// Reordering still works for this session when browser storage is unavailable.
			}
		}
	}

	#loadPluginOrder(workspaceId: string | null) {
		if (workspaceId === this.#orderWorkspaceId) return;
		this.#orderWorkspaceId = workspaceId;
		if (!browser || !workspaceId) {
			this.pluginOrder = normalizePluginOrder();
			return;
		}

		try {
			const saved = JSON.parse(
				localStorage.getItem(`${PLUGIN_ORDER_STORAGE_PREFIX}${workspaceId}`) ??
					'null',
			);
			this.pluginOrder = normalizePluginOrder(
				Array.isArray(saved) ? saved : null,
			);
		} catch {
			this.pluginOrder = normalizePluginOrder();
		}
	}

	async setEnabled(pluginId: string, enabled: boolean) {
		const reason = this.unavailableReason(pluginId);
		if (reason) throw new Error(reason);
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
