import { browser } from '$app/environment';
import type { Diagnostic, WorkspaceState } from '@noura/workspace';
import { createNouraClient, createTauriTransport } from '@noura/workspace';

let client: ReturnType<typeof createNouraClient> | null = null;

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
	if (!client) client = createNouraClient(createTauriTransport());
	return client;
}

class WorkspaceStore {
	private data = $state<WorkspaceState | null>(null);
	private loadingState = $state(false);
	private errorMessage = $state<string | null>(null);
	private ready = $state(false);
	private recentItems = $state.raw<RecentWorkspace[]>([]);

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

	async init() {
		if (this.ready) return;
		this.ready = true;
		await this.refresh();
		if (this.isIdle) await this.refreshRecents();
	}

	async refresh() {
		if (!browser) return;
		try {
			this.loadingState = true;
			this.errorMessage = null;
			this.data = await getNouraClient().workspaces.current();
		} catch (error) {
			this.errorMessage = errorMessage(error);
		} finally {
			this.loadingState = false;
		}
	}

	async refreshRecents() {
		if (!browser) return;
		try {
			this.recentItems = await getNouraClient().workspaces.listRecent();
		} catch (error) {
			this.errorMessage = errorMessage(error);
		}
	}

	async open(path: string) {
		this.loadingState = true;
		this.errorMessage = null;
		try {
			this.data = await getNouraClient().workspaces.open({ path });
		} catch (error) {
			this.errorMessage = errorMessage(error);
		} finally {
			this.loadingState = false;
		}
	}

	async pickAndOpen() {
		this.loadingState = true;
		this.errorMessage = null;
		try {
			const path = await getNouraClient().workspaces.pickFolder({
				title: 'Open a Noura workspace',
			});
			if (path) this.data = await getNouraClient().workspaces.open({ path });
		} catch (error) {
			this.errorMessage = errorMessage(error);
		} finally {
			this.loadingState = false;
		}
	}

	async create(path: string, name: string) {
		this.loadingState = true;
		this.errorMessage = null;
		try {
			this.data = await getNouraClient().workspaces.create({ path, name });
		} catch (error) {
			this.errorMessage = errorMessage(error);
		} finally {
			this.loadingState = false;
		}
	}

	async pickAndCreate(name: string) {
		this.loadingState = true;
		this.errorMessage = null;
		try {
			const path = await getNouraClient().workspaces.pickFolder({
				title: 'Choose or create a workspace folder',
			});
			if (path) {
				this.data = await getNouraClient().workspaces.create({ path, name });
			}
		} catch (error) {
			this.errorMessage = errorMessage(error);
		} finally {
			this.loadingState = false;
		}
	}
}

export const workspace = new WorkspaceStore();

class DiagnosticsStore {
	private items = $state<Diagnostic[]>([]);

	async refresh() {
		if (!browser) return;
		this.items = workspace.state?.diagnostics ?? [];
	}

	get issues() {
		return this.items;
	}
	get all() {
		return this.items;
	}
}

export const diagnostics = new DiagnosticsStore();
