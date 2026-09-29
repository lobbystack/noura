import { browser } from '$app/environment';
import { SvelteSet } from 'svelte/reactivity';
import type { CoreEvent } from '@noura/workspace';
import {
	ancestorFolders,
	buildWorkspaceTree,
	collectFolderPaths,
	findTreeNode,
	moveTreePath,
	objectChangesKeepTree,
	remapPath,
	type WorkspaceTreeNode,
} from './workspace-tree';
import { LiveRefresh } from './live-refresh';
import {
	BULK_OBJECT_EVENT,
	objectEvents,
	type ObjectEventPayload,
} from './object-events';
import { getNouraClient, workspace } from './state.svelte';

const EXPANDED_STORAGE_PREFIX = 'noura.file-tree.expanded.v1:';

/**
 * Reactive projection of the workspace folder tree for the sidebar.
 * The canonical sources are the workspace files; core events either patch
 * the tree in place or trigger one debounced read of the file list.
 */
class WorkspaceTreeStore {
	/** Replaced as a whole on every change, never mutated in place. */
	tree = $state.raw<WorkspaceTreeNode[]>([]);
	/** Folder paths currently open, remembered per workspace on this device. */
	expanded = new SvelteSet<string>();
	loading = $state(false);
	/** The row that has keyboard focus and receives tree commands. */
	selectedPath = $state<string | null>(null);
	/** The row showing an inline rename field. */
	renamingPath = $state<string | null>(null);

	#refresher = new LiveRefresh({
		refresh: () => this.refresh(),
		delayMs: 150,
	});
	#refreshSequence = 0;
	#workspaceId: string | null | undefined;
	#started = false;

	async start(workspaceId: string | null | undefined): Promise<void> {
		if (!browser) return;
		// The sidebar remounts on every visit to a route that shows it. Once the
		// tree runs for this workspace, events and window focus keep it
		// current, so remounting must not read the workspace again.
		if (this.#started && workspaceId === this.#workspaceId) return;
		this.#workspaceChanged(workspaceId);
		if (!this.#started) {
			this.#started = true;
			// The store lives as long as the app, so the subscription does too.
			void getNouraClient()
				.events.subscribe((event) => this.#handleEvent(event))
				.catch(() => {
					// Without events, window focus still refreshes the tree.
				});
			window.addEventListener('focus', this.#invalidate);
			document.addEventListener(
				'visibilitychange',
				this.#invalidateWhenVisible,
			);
		}
		await this.#refresher.refreshNow();
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
				this.#pruneExpanded();
			}
		} catch {
			// No workspace open (or transient failure): keep the last tree
			// rather than flashing the explorer empty.
		} finally {
			if (sequence === this.#refreshSequence) this.loading = false;
		}
	}

	/** Show a move right away; the next read of the file list confirms it. */
	applyMove(from: string, to: string) {
		const next = moveTreePath(this.tree, from, to);
		if (next) this.tree = next;
		this.#remapState(from, to);
	}

	toggle(path: string) {
		if (this.expanded.has(path)) {
			this.expanded.delete(path);
		} else {
			this.expanded.add(path);
		}
		this.#saveExpanded();
	}

	setExpanded(path: string, open: boolean) {
		if (open === this.expanded.has(path)) return;
		this.toggle(path);
	}

	isExpanded(path: string): boolean {
		return this.expanded.has(path);
	}

	/** Open `path` (when it is a folder) and every folder above it. */
	expandTo(path: string) {
		const folders = ancestorFolders(path);
		if (path && findTreeNode(this.tree, path)?.kind !== 'file')
			folders.push(path);
		let changed = false;
		for (const folder of folders) {
			if (!this.expanded.has(folder)) {
				this.expanded.add(folder);
				changed = true;
			}
		}
		if (changed) this.#saveExpanded();
	}

	collapseAll() {
		if (this.expanded.size === 0) return;
		this.expanded.clear();
		this.#saveExpanded();
	}

	/** The folder new notes and folders go into: the selected folder or its parent. */
	get targetFolder(): string {
		const path = this.selectedPath;
		if (!path) return '';
		const node = findTreeNode(this.tree, path);
		if (!node) return '';
		if (node.kind === 'folder') return node.relativePath;
		const cut = node.relativePath.lastIndexOf('/');
		return cut === -1 ? '' : node.relativePath.slice(0, cut);
	}

	#invalidate = () => {
		if (this.#workspaceId) this.#refresher.invalidate();
	};

	#invalidateWhenVisible = () => {
		if (document.visibilityState === 'visible') this.#invalidate();
	};

	/**
	 * Core events are hints. Saving a document's content never changes the
	 * tree, so those events are ignored; moves patch the tree in place; any
	 * other change reads the file list again.
	 */
	#handleEvent(event: CoreEvent) {
		if (!this.#workspaceId || event.workspaceId !== this.#workspaceId) return;
		const payload = (event.payload ?? {}) as ObjectEventPayload & {
			paths?: unknown;
		};
		switch (event.type) {
			case 'object:updated':
			case 'object:created': {
				const change = { ...payload, type: event.type };
				if (!objectChangesKeepTree(this.tree, [change])) this.#invalidate();
				return;
			}
			case BULK_OBJECT_EVENT: {
				// A bulk change is one read of the file list at most, never one
				// per object.
				const changes = objectEvents(event).map((change) => ({
					...(change.payload as ObjectEventPayload),
					type: change.type,
				}));
				if (!objectChangesKeepTree(this.tree, changes)) this.#invalidate();
				return;
			}
			case 'object:moved': {
				if (payload.previousPath && payload.path) {
					const next = moveTreePath(
						this.tree,
						payload.previousPath,
						payload.path,
					);
					if (next) {
						this.tree = next;
						this.#remapState(payload.previousPath, payload.path);
						return;
					}
				}
				this.#invalidate();
				return;
			}
			case 'file:changed': {
				const paths = Array.isArray(payload.paths) ? payload.paths : [];
				const contentOnly =
					event.source === 'application' &&
					paths.length > 0 &&
					paths.every(
						(path) =>
							typeof path === 'string' &&
							findTreeNode(this.tree, path) !== null,
					);
				if (!contentOnly) this.#invalidate();
				return;
			}
			case 'object:deleted':
			case 'workspace:ready':
			case 'workspace:manifest-updated':
				this.#invalidate();
				return;
			default:
				return;
		}
	}

	#remapState(from: string, to: string) {
		const moved = [...this.expanded]
			.map((path) => [path, remapPath(path, from, to)] as const)
			.filter(([, next]) => next !== null);
		for (const [path] of moved) this.expanded.delete(path);
		for (const [, next] of moved) this.expanded.add(next!);
		if (moved.length > 0) this.#saveExpanded();
		if (this.selectedPath)
			this.selectedPath =
				remapPath(this.selectedPath, from, to) ?? this.selectedPath;
	}

	/** Clear an outdated projection before reading the newly active workspace. */
	#workspaceChanged(workspaceId: string | null | undefined): void {
		if (workspaceId === this.#workspaceId) return;
		this.#workspaceId = workspaceId;
		this.#refreshSequence += 1;
		this.tree = [];
		this.selectedPath = null;
		this.renamingPath = null;
		this.expanded.clear();
		for (const path of this.#readExpanded(workspaceId)) this.expanded.add(path);
		this.loading = workspaceId !== null && workspaceId !== undefined;
	}

	#readExpanded(workspaceId: string | null | undefined): string[] {
		if (!workspaceId) return [];
		try {
			const saved = JSON.parse(
				localStorage.getItem(`${EXPANDED_STORAGE_PREFIX}${workspaceId}`) ??
					'[]',
			);
			return Array.isArray(saved)
				? saved.filter((path): path is string => typeof path === 'string')
				: [];
		} catch {
			return [];
		}
	}

	#saveExpanded() {
		if (!browser || !this.#workspaceId) return;
		try {
			localStorage.setItem(
				`${EXPANDED_STORAGE_PREFIX}${this.#workspaceId}`,
				JSON.stringify([...this.expanded]),
			);
		} catch {
			// The tree still works for this session when storage is unavailable.
		}
	}

	/** Drop open folders that no longer exist (moves, deletes, reloads). */
	#pruneExpanded() {
		const known = collectFolderPaths(this.tree);
		let changed = false;
		for (const path of this.expanded) {
			if (!known.has(path)) {
				this.expanded.delete(path);
				changed = true;
			}
		}
		if (changed) this.#saveExpanded();
		if (this.selectedPath && !findTreeNode(this.tree, this.selectedPath))
			this.selectedPath = null;
	}
}

export const workspaceTree = new WorkspaceTreeStore();
