import { browser } from '$app/environment';
import type { WorkspaceState } from '@noura/workspace';
import { createNouraClient, createTauriTransport } from '@noura/workspace';
import { AppPluginRuntime } from './app-plugin-runtime';
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
	if (getAppPlatform() === 'web')
		throw new Error(
			'Local workspaces are not available in the web browser. Open Noura in the native app.',
		);
	if (!client) client = createNouraClient(createTauriTransport());
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

class WorkspaceStore {
	private data = $state<WorkspaceState | null>(null);
	private loadingState = $state(false);
	private errorMessage = $state<string | null>(null);
	private ready = $state(false);
	private recentItems = $state.raw<RecentWorkspace[]>([]);
	/** The newest workspace action owns the visible projection. */
	#requestSequence = 0;
	#recentRequestSequence = 0;

	get state() {
		return this.data;
	}
	get isReady() {
		return this.data?.phase === 'ready';
	}
	get isIdle() {
		return this.data?.phase === 'idle';
	}
	get isLoading() {
		return this.loadingState;
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
	get name() {
		const workspaceId = this.data?.workspaceId;
		const current = workspaceId
			? this.recentItems.find((recent) => recent.workspaceId === workspaceId)
			: undefined;
		if (current) return current.name;

		const rootPath = this.data?.rootPath;
		if (rootPath) {
			const segments = rootPath.split(/[\\/]/).filter(Boolean);
			return segments.at(-1) ?? 'Noura';
		}

		return 'Noura';
	}

	async init() {
		if (this.ready) return;
		this.ready = true;
		await this.refresh();
		await this.refreshRecents();
	}

	async refresh() {
		if (!browser) return;
		const requestSequence = ++this.#requestSequence;
		try {
			this.loadingState = true;
			this.errorMessage = null;
			const data = await getNouraClient().workspaces.current();
			if (requestSequence === this.#requestSequence) this.data = data;
		} catch (error) {
			if (requestSequence === this.#requestSequence)
				this.errorMessage = errorMessage(error);
		} finally {
			if (requestSequence === this.#requestSequence) this.loadingState = false;
		}
	}

	async refreshRecents() {
		if (!browser) return;
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

	async open(path: string): Promise<boolean> {
		const requestSequence = ++this.#requestSequence;
		this.loadingState = true;
		this.errorMessage = null;
		try {
			const data = await getNouraClient().workspaces.open({ path });
			if (requestSequence !== this.#requestSequence) return false;
			this.data = data;
			await this.refreshRecents();
			return true;
		} catch (error) {
			if (requestSequence === this.#requestSequence)
				this.errorMessage = errorMessage(error);
			return false;
		} finally {
			if (requestSequence === this.#requestSequence) this.loadingState = false;
		}
	}

	async pickAndOpen() {
		const requestSequence = ++this.#requestSequence;
		this.loadingState = true;
		this.errorMessage = null;
		try {
			const path = await getNouraClient().workspaces.pickFolder({
				title: 'Open a Noura workspace',
			});
			if (!path || requestSequence !== this.#requestSequence) return;
			const data = await getNouraClient().workspaces.open({ path });
			if (requestSequence === this.#requestSequence) {
				this.data = data;
				await this.refreshRecents();
			}
		} catch (error) {
			if (requestSequence === this.#requestSequence)
				this.errorMessage = errorMessage(error);
		} finally {
			if (requestSequence === this.#requestSequence) this.loadingState = false;
		}
	}

	async create(path: string, name: string) {
		const requestSequence = ++this.#requestSequence;
		this.loadingState = true;
		this.errorMessage = null;
		try {
			const data = await getNouraClient().workspaces.create({ path, name });
			if (requestSequence === this.#requestSequence) {
				this.data = data;
				await this.refreshRecents();
			}
		} catch (error) {
			if (requestSequence === this.#requestSequence)
				this.errorMessage = errorMessage(error);
		} finally {
			if (requestSequence === this.#requestSequence) this.loadingState = false;
		}
	}

	async pickAndCreate(name: string) {
		const requestSequence = ++this.#requestSequence;
		this.loadingState = true;
		this.errorMessage = null;
		try {
			const path = await getNouraClient().workspaces.pickFolder({
				title: 'Choose or create a workspace folder',
			});
			if (!path || requestSequence !== this.#requestSequence) return;
			const data = await getNouraClient().workspaces.create({ path, name });
			if (requestSequence === this.#requestSequence) {
				this.data = data;
				await this.refreshRecents();
			}
		} catch (error) {
			if (requestSequence === this.#requestSequence)
				this.errorMessage = errorMessage(error);
		} finally {
			if (requestSequence === this.#requestSequence) this.loadingState = false;
		}
	}
}

export const workspace = new WorkspaceStore();

class DiagnosticsStore {
	async refresh() {
		if (!browser) return;
		await workspace.refresh();
	}

	get issues() {
		return workspace.state?.diagnostics ?? [];
	}
	get all() {
		return workspace.state?.diagnostics ?? [];
	}
}

export const diagnostics = new DiagnosticsStore();
