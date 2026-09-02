import { browser } from '$app/environment';
import type { CoreEvent } from '@noura/workspace';
import { buildWorkspaceTree, type WorkspaceTreeNode } from './workspace-tree';
import { getNouraClient } from './state.svelte';

const REFRESH_DEBOUNCE_MS = 250;
const LIVE_EVENTS = new Set([
	'object:created',
	'object:updated',
	'object:deleted',
	'object:moved',
	'file:changed',
	'search:index-updated',
]);

/**
 * Reactive projection of the workspace folder tree for the sidebar.
 * The canonical sources are the workspace files; this store refetches
 * on the core events that follow any external or managed change.
 */
class WorkspaceTreeStore {
	tree = $state<WorkspaceTreeNode[]>([]);
	/** Folder paths currently expanded, collapsed back to empty on reload. */
	expanded = $state<Set<string>>(new Set());
	loading = $state(false);

	#started = false;
	#refreshTimer: ReturnType<typeof setTimeout> | undefined;

	async start(): Promise<void> {
		if (!browser || this.#started) return;
		this.#started = true;
		await this.refresh();
		try {
			await getNouraClient().events.subscribe((event: CoreEvent) => {
				if (!LIVE_EVENTS.has(event.type)) return;
				clearTimeout(this.#refreshTimer);
				this.#refreshTimer = setTimeout(() => {
					void this.refresh();
				}, REFRESH_DEBOUNCE_MS);
			});
		} catch {
			this.#started = false;
		}
	}

	async refresh(): Promise<void> {
		if (!browser) return;
		this.loading = true;
		try {
			const entries = await getNouraClient().files.list();
			this.tree = buildWorkspaceTree(entries);
			this.pruneExpanded();
		} catch {
			// No workspace open (or transient failure): keep the last tree
			// rather than flashing the explorer empty.
		} finally {
			this.loading = false;
		}
	}

	toggle(path: string) {
		const next = new Set(this.expanded);
		if (next.has(path)) {
			next.delete(path);
		} else {
			next.add(path);
		}
		this.expanded = next;
	}

	isExpanded(path: string): boolean {
		return this.expanded.has(path);
	}

	expandTo(path: string) {
		if (this.expanded.has(path)) return;
		const next = new Set(this.expanded);
		next.add(path);
		this.expanded = next;
	}

	/** Drop expanded folders that no longer exist (moves, deletes, reloads). */
	private pruneExpanded() {
		const known = new Set<string>();
		const walk = (nodes: WorkspaceTreeNode[]) => {
			for (const node of nodes) {
				if (node.kind === 'folder') {
					known.add(node.relativePath);
					walk(node.children);
				}
			}
		};
		walk(this.tree);
		for (const path of this.expanded) {
			if (!known.has(path)) this.expanded.delete(path);
		}
	}
}

export const workspaceTree = new WorkspaceTreeStore();
