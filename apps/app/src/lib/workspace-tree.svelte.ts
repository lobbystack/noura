import { browser } from '$app/environment';
import { SvelteSet } from 'svelte/reactivity';
import { buildWorkspaceTree, type WorkspaceTreeNode } from './workspace-tree';
import { LiveProjection } from './live-refresh';
import { getNouraClient, workspace } from './state.svelte';

/**
 * Reactive projection of the workspace folder tree for the sidebar.
 * The canonical sources are the workspace files; this store refetches
 * on the core events that follow any external or managed change.
 */
class WorkspaceTreeStore {
	tree = $state<WorkspaceTreeNode[]>([]);
	/** Folder paths currently expanded, collapsed back to empty on reload. */
	expanded = new SvelteSet<string>();
	loading = $state(false);

	#projection: LiveProjection | undefined;
	#refreshSequence = 0;
	#workspaceId: string | null | undefined;

	async start(workspaceId: string | null | undefined): Promise<void> {
		if (!browser) return;
		this.#workspaceChanged(workspaceId);
		this.#projection ??= new LiveProjection({
			refresh: () => this.refresh(),
			subscribe: (handler) => getNouraClient().events.subscribe(handler),
			workspaceId: () => workspace.state?.workspaceId,
			focusSource: window,
			visibilitySource: document,
		});
		await this.#projection.start();
	}

	async refresh(): Promise<void> {
		if (!browser) return;
		const sequence = ++this.#refreshSequence;
		const workspaceId = workspace.state?.workspaceId;
		this.loading = true;
		try {
			const entries = await getNouraClient().files.list();
			if (
				sequence === this.#refreshSequence &&
				workspace.state?.workspaceId === workspaceId
			) {
				this.tree = buildWorkspaceTree(entries);
				this.pruneExpanded();
			}
		} catch {
			// No workspace open (or transient failure): keep the last tree
			// rather than flashing the explorer empty.
		} finally {
			if (sequence === this.#refreshSequence) this.loading = false;
		}
	}

	/** Clear an outdated projection before reading the newly active workspace. */
	#workspaceChanged(workspaceId: string | null | undefined): void {
		if (workspaceId === this.#workspaceId) return;
		this.#workspaceId = workspaceId;
		this.#refreshSequence += 1;
		this.tree = [];
		this.expanded.clear();
		if (workspaceId === null || workspaceId === undefined) {
			this.loading = false;
			return;
		}
		this.loading = true;
	}

	toggle(path: string) {
		if (this.expanded.has(path)) {
			this.expanded.delete(path);
		} else {
			this.expanded.add(path);
		}
	}

	isExpanded(path: string): boolean {
		return this.expanded.has(path);
	}

	expandTo(path: string) {
		if (this.expanded.has(path)) return;
		this.expanded.add(path);
	}

	/** Drop expanded folders that no longer exist (moves, deletes, reloads). */
	private pruneExpanded() {
		const known = new SvelteSet<string>();
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
