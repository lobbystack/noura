import type { Diagnostic, WorkspaceState } from '@noura/workspace';
import type { BrowserWorkspaceSnapshot } from '@noura/browser-workspace';
import { createNouraClient, createTauriTransport } from '@noura/workspace';
import { AppPluginRuntime } from './app-plugin-runtime';
import {
	getBrowserWorkspace,
	lastBrowserWorkspace,
	NEW_BROWSER_WORKSPACE_PATH,
	rememberBrowserWorkspace,
} from './browser-workspace';
import { shareEventSubscription } from './live-refresh';
import { getAppPlatform } from './platform';

let client: ReturnType<typeof createNouraClient> | null = null;
let pluginRuntime: AppPluginRuntime | null = null;

type RecentWorkspace = {
	path: string;
	name: string;
	workspaceId: string;
};

function errorMessage(error: unknown) {
	if (error instanceof Error) return error.message;
	if (error && typeof error === 'object' && 'message' in error) {
		return String(error.message);
	}
	return String(error);
}

export function getNouraClient() {
	if (!getAppPlatform())
		throw new Error(
			'Native platform could not be detected. Start or build the frontend through the Tauri CLI.',
		);
	if (!client) {
		// The browser keeps workspaces in its own storage behind the same
		// typed client the desktop app uses.
		const transport =
			getAppPlatform() === 'web'
				? getBrowserWorkspace().transport
				: createTauriTransport();
		client = createNouraClient({
			...transport,
			subscribe: shareEventSubscription(transport.subscribe),
		});
	}
	return client;
}

export function getPluginRuntime(): AppPluginRuntime {
	const platform = getAppPlatform();
	if (!platform)
		throw new Error(
			'Native platform could not be detected. Start or build the frontend through the Tauri CLI.',
		);
	if (!pluginRuntime)
		pluginRuntime = new AppPluginRuntime(getNouraClient(), platform);
	return pluginRuntime;
}

/** Phases in which the engine is still opening or rebuilding a workspace. */
const TRANSITIONAL_PHASES = new Set<WorkspaceState['phase']>([
	'opening',
	'scanning',
	'indexing',
	'rebuilding',
]);
const SETTLED_PHASES = new Set<WorkspaceState['phase']>([
	'ready',
	'idle',
	'failed',
]);
/** Workspace lifecycle events that can change what `workspace_state` returns. */
const LIFECYCLE_EVENTS = new Set([
	'workspace:ready',
	'workspace:closed',
	'workspace:failed',
]);
const TRANSITION_POLL_MS = 400;

class WorkspaceStore {
	private data = $state.raw<WorkspaceState | null>(null);
	private loadingState = $state(false);
	private errorMessage = $state<string | null>(null);
	private ready = $state(false);
	private recentItems = $state.raw<RecentWorkspace[]>([]);
	/** The newest workspace action owns the visible projection. */
	#requestSequence = 0;
	#recentRequestSequence = 0;
	/** User actions in flight. Lifecycle events are ignored meanwhile: the
	 * action's own result is newer than anything a refresh could read. */
	#actions = 0;
	#pollTimer: ReturnType<typeof setTimeout> | undefined;

	get state() {
		return this.data;
	}
	get isReady() {
		return this.data?.phase === 'ready';
	}
	get isIdle() {
		return this.data?.phase === 'idle';
	}
	/** True while an action runs or the engine is still opening a workspace. */
	get isLoading() {
		return (
			this.loadingState ||
			(this.data !== null && TRANSITIONAL_PHASES.has(this.data.phase))
		);
	}
	get error() {
		return this.errorMessage;
	}
	get initialized() {
		return this.ready;
	}
	get recents() {
		return this.recentItems;
	}
	/**
	 * Changes only when the workspace settles into another phase or ID.
	 * Effects key on this instead of `state`, which is replaced on every read.
	 */
	get settledKey(): string | null {
		const data = this.data;
		if (!data || !SETTLED_PHASES.has(data.phase)) return null;
		return `${data.phase}:${data.workspaceId ?? ''}`;
	}
	get name() {
		const workspaceId = this.data?.workspaceId;
		const current = workspaceId
			? this.recentItems.find((recent) => recent.workspaceId === workspaceId)
			: undefined;
		if (current) return current.name;

		const rootPath = this.data?.rootPath;
		if (rootPath) {
			const segments = rootPath.split(/[\\/]/).filter(Boolean);
			return segments.at(-1) ?? 'noura';
		}

		return 'noura';
	}

	async init() {
		if (this.ready) return;
		this.ready = true;
		// Subscribe before the first read so a workspace that finishes opening
		// in the background cannot slip between the read and the listener.
		try {
			await getNouraClient().events.subscribe((event) => {
				if (LIFECYCLE_EVENTS.has(event.type) && this.#actions === 0)
					void this.refresh();
			});
		} catch {
			// The transition poll in #adopt still settles the state.
		}
		await Promise.all([this.refresh(), this.refreshRecents()]);
		if (getAppPlatform() === 'web') await this.#reopenBrowserWorkspace();
	}

	/**
	 * The desktop app reopens the last workspace itself. A browser tab starts
	 * with none open, so open the one this browser used last.
	 */
	async #reopenBrowserWorkspace() {
		if (this.data?.phase !== 'idle') return;
		const last = lastBrowserWorkspace();
		const recent = this.recentItems.find((item) => item.workspaceId === last);
		if (recent) await this.open(recent.path);
	}

	async refresh() {
		const requestSequence = ++this.#requestSequence;
		try {
			this.loadingState = true;
			this.errorMessage = null;
			const data = await getNouraClient().workspaces.current();
			if (requestSequence === this.#requestSequence) this.#adopt(data);
		} catch (error) {
			if (requestSequence === this.#requestSequence)
				this.errorMessage = errorMessage(error);
		} finally {
			if (requestSequence === this.#requestSequence) this.loadingState = false;
		}
	}

	async refreshRecents() {
		const requestSequence = ++this.#recentRequestSequence;
		try {
			const recents = await getNouraClient().workspaces.listRecent();
			if (requestSequence === this.#recentRequestSequence)
				this.recentItems = recents;
		} catch (error) {
			if (requestSequence === this.#recentRequestSequence)
				this.errorMessage = errorMessage(error);
		}
	}

	/** Remove a workspace from the recent list. Its folder stays as it is. */
	async forgetRecent(workspaceId: string) {
		const requestSequence = ++this.#recentRequestSequence;
		const recents = await getNouraClient().workspaces.forgetRecent({
			workspaceId,
		});
		if (requestSequence === this.#recentRequestSequence)
			this.recentItems = recents;
	}

	/** Show the open workspace's folder in the system file manager. */
	async reveal() {
		await getNouraClient().workspaces.showInFolder('root');
	}

	/** Open the failed or last used workspace again. */
	async retry() {
		const path = this.data?.rootPath ?? this.recentItems[0]?.path;
		if (path) await this.open(path);
		else await this.refresh();
	}

	async open(path: string): Promise<boolean> {
		return this.#run(async (isCurrent) => {
			const data = await getNouraClient().workspaces.open({ path });
			if (!isCurrent()) return false;
			this.#adopt(data);
			void this.refreshRecents();
			return true;
		}, false);
	}

	async pickAndOpen() {
		await this.#run(async (isCurrent) => {
			const path = await getNouraClient().workspaces.pickFolder({
				title: 'Open a folder',
			});
			if (!path || !isCurrent()) return;
			const data = await getNouraClient().workspaces.open({ path });
			if (!isCurrent()) return;
			this.#adopt(data);
			void this.refreshRecents();
		}, undefined);
	}

	async create(path: string, name: string) {
		await this.#run(async (isCurrent) => {
			const data = await getNouraClient().workspaces.create({ path, name });
			if (!isCurrent()) return;
			this.#adopt(data);
			void this.refreshRecents();
		}, undefined);
	}

	/**
	 * Create a workspace called `name`: in a folder the user picks on desktop,
	 * or in this browser's storage on the web.
	 */
	async createNamed(name: string) {
		if (getAppPlatform() === 'web')
			await this.create(NEW_BROWSER_WORKSPACE_PATH, name);
		else await this.pickAndCreate(name);
	}

	async pickAndCreate(name: string) {
		await this.#run(async (isCurrent) => {
			const path = await getNouraClient().workspaces.pickFolder({
				title: 'Choose a folder for the new workspace',
			});
			if (!path || !isCurrent()) return;
			const data = await getNouraClient().workspaces.create({ path, name });
			if (!isCurrent()) return;
			this.#adopt(data);
			void this.refreshRecents();
		}, undefined);
	}

	/**
	 * Open a restored browser backup as a new workspace. Rejects without
	 * changing anything when the backup can't be restored here.
	 */
	async importBrowserBackup(snapshot: BrowserWorkspaceSnapshot) {
		const requestSequence = ++this.#requestSequence;
		this.#actions += 1;
		this.loadingState = true;
		try {
			const data = await getBrowserWorkspace().importWorkspace(snapshot);
			if (requestSequence === this.#requestSequence) this.#adopt(data);
			void this.refreshRecents();
		} finally {
			this.#actions -= 1;
			if (requestSequence === this.#requestSequence) this.loadingState = false;
		}
	}

	/** Runs a user action as the newest owner of the visible projection. */
	async #run<T>(
		action: (isCurrent: () => boolean) => Promise<T>,
		fallback: T,
	): Promise<T> {
		const requestSequence = ++this.#requestSequence;
		const isCurrent = () => requestSequence === this.#requestSequence;
		this.#actions += 1;
		this.loadingState = true;
		this.errorMessage = null;
		try {
			return await action(isCurrent);
		} catch (error) {
			if (isCurrent()) this.errorMessage = errorMessage(error);
			return fallback;
		} finally {
			this.#actions -= 1;
			if (isCurrent()) this.loadingState = false;
		}
	}

	#adopt(data: WorkspaceState) {
		this.data = data;
		if (
			getAppPlatform() === 'web' &&
			data.phase === 'ready' &&
			data.workspaceId
		)
			rememberBrowserWorkspace(data.workspaceId);
		diagnostics.adopt(data);
		clearTimeout(this.#pollTimer);
		// A background open reports progress through events. Poll as well, so
		// a missed event can never leave the loading screen up.
		if (TRANSITIONAL_PHASES.has(data.phase))
			this.#pollTimer = setTimeout(() => {
				if (this.#actions === 0) void this.refresh();
			}, TRANSITION_POLL_MS);
	}
}

export const workspace = new WorkspaceStore();

/**
 * Workspace diagnostics for the Home page. They are read on their own, so
 * refreshing them never replaces `workspace.state`.
 */
class DiagnosticsStore {
	private items = $state.raw<Diagnostic[]>([]);

	get issues() {
		return this.items;
	}

	adopt(data: WorkspaceState) {
		this.items = data.diagnostics;
	}

	async refresh() {
		const data = await getNouraClient().workspaces.current();
		if (data.workspaceId === workspace.state?.workspaceId)
			this.items = data.diagnostics;
	}
}

export const diagnostics = new DiagnosticsStore();
