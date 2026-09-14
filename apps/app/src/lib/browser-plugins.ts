import {
	firstPartyPlugins,
	type PluginRuntime,
	type NouraClient,
} from '@noura/workspace/browser';
import type { PluginSettingsModel } from './plugin-settings-model';

function message(cause: unknown): string {
	return cause && typeof cause === 'object' && 'message' in cause
		? String(cause.message)
		: String(cause);
}

/** UI projection only. Canonical preferences and activation belong to the shared runtime. */
export function createBrowserPluginModel(
	client: NouraClient,
	runtime: PluginRuntime,
) {
	let revision: string | null = null;
	let enabledIds: string[] = [];
	let activeIds: string[] = [];
	let ready = false;
	let synced = false;
	let lastError: string | null = null;
	let disposed = false;
	let chain = Promise.resolve();
	let unlisten: (() => void) | undefined;
	const listeners = new Set<(model: PluginSettingsModel) => void>();
	const catalog = firstPartyPlugins.map((plugin) => plugin.manifest);
	const supported = (id: string) => {
		const plugin = firstPartyPlugins.find(
			(plugin) => plugin.manifest.id === id,
		);
		return !!plugin && !runtime.host.activationError(plugin);
	};
	function snapshot(): PluginSettingsModel {
		const enabled = enabledIds;
		const active = activeIds;
		const loaded = synced;
		const open = ready;
		const error = lastError;
		return {
			platform: 'web',
			catalog,
			orderedPluginIds: catalog.map((item) => item.id),
			enabledIds: enabled,
			synced: loaded,
			lastError: error,
			isSupported: supported,
			isEnabled: (id) =>
				open && loaded && !error && enabled.includes(id) && active.includes(id),
			unavailableReason: (id) =>
				!supported(id)
					? 'Not available in browser workspaces (platform or capability unsupported).'
					: !open
						? 'Open a workspace to manage this plugin.'
						: !loaded
							? 'Loading workspace plugin preferences…'
							: error
								? 'Runtime needs reconciliation. Refresh plugins to retry.'
								: null,
			sync,
			setEnabled,
		};
	}
	function publish() {
		if (!disposed) for (const listener of listeners) listener(snapshot());
	}
	function enqueue(action: () => Promise<void>) {
		const run = chain.then(async () => {
			if (!disposed) await action();
		});
		chain = run.catch(() => {});
		return run;
	}
	async function reconcile() {
		try {
			ready = (await client.workspaces.current()).phase === 'ready';
			if (ready) {
				// A failed activation must not hide a preference that reached disk.
				const saved = await runtime.registry.read();
				revision = saved.updated;
				enabledIds = saved.enabledPluginIds;
				await runtime.syncWithManifest();
				const preference = await runtime.registry.read();
				revision = preference.updated;
				enabledIds = preference.enabledPluginIds;
				activeIds = runtime.host.activeManifests().map((item) => item.id);
			} else {
				await runtime.deactivateAll();
				revision = null;
				enabledIds = [];
				activeIds = [];
			}
			lastError = null;
		} catch (cause) {
			activeIds = [];
			lastError = `Plugin reconciliation failed: ${message(cause)}. Preferences may already be saved. Refresh plugins to retry; drafts are retained.`;
		} finally {
			synced = true;
			publish();
		}
	}
	function sync() {
		return enqueue(reconcile);
	}
	function setEnabled(id: string, enabled: boolean) {
		// Capture the revision the user saw, not a new revision read after a queued sync.
		const expected = revision;
		const wasEnabled = enabledIds.includes(id);
		return enqueue(async () => {
			if (!expected || !ready || !supported(id))
				throw new Error('Plugin is unavailable.');
			let committed: string | null = null;
			try {
				const preference = await runtime.registry.setEnabled(
					id,
					enabled,
					expected,
				);
				committed = preference.updated;
				await runtime.syncWithManifest();
			} catch (cause) {
				const conflict =
					cause &&
					typeof cause === 'object' &&
					'code' in cause &&
					['manifest_conflict', 'revision_conflict'].includes(
						String(cause.code),
					);
				let outcome = conflict
					? 'Nothing was overwritten. Refresh plugins, review the current preference and retry.'
					: 'The preference write did not return confirmation. No rollback was attempted without a confirmed commit revision. Review the reloaded preference below.';
				if (committed) {
					try {
						await runtime.registry.setEnabled(id, wasEnabled, committed);
						outcome =
							'The previous preference was restored with a revision-checked write.';
					} catch {
						outcome =
							'Rollback could not be confirmed (the manifest may have changed elsewhere). The saved preference may differ from the requested state.';
					}
				}
				await reconcile();
				throw new Error(
					`${message(cause)}. ${outcome} Drafts are retained.${lastError ? ` ${lastError}` : ''}`,
				);
			}
			await reconcile();
		});
	}
	return {
		snapshot,
		sync,
		subscribe(listener: (model: PluginSettingsModel) => void) {
			listeners.add(listener);
			listener(snapshot());
			return () => {
				listeners.delete(listener);
			};
		},
		async init() {
			const stop = await client.events.subscribe((event) => {
				if (event.type === 'workspace:manifest-updated') void sync();
			});
			if (disposed) stop();
			else unlisten = stop;
			await sync();
		},
		/** Never carry registrations from one workspace into another. */
		scope(action: () => Promise<void>) {
			return enqueue(async () => {
				ready = false;
				activeIds = [];
				publish();
				await runtime.deactivateAll();
				try {
					await action();
				} finally {
					await reconcile();
				}
			});
		},
		async dispose() {
			disposed = true;
			unlisten?.();
			listeners.clear();
			await chain;
			await runtime.deactivateAll();
		},
	};
}
