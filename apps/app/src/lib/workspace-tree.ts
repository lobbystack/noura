import type { WorkspaceEntry } from '@noura/workspace';

export interface WorkspaceTreeNode {
	name: string;
	relativePath: string;
	kind: WorkspaceEntry['kind'];
	objectId: string | null;
	objectType: string | null;
	parseStatus: WorkspaceEntry['parseStatus'];
	children: WorkspaceTreeNode[];
}

const ROUTES_BY_TYPE: Record<string, string> = {
	note: '/notes',
	task: '/tasks',
	project: '/projects',
};

function compareNodes(
	left: WorkspaceTreeNode,
	right: WorkspaceTreeNode,
): number {
	const kindRank = (node: WorkspaceTreeNode) =>
		node.kind === 'folder' ? 0 : 1;
	return (
		kindRank(left) - kindRank(right) ||
		left.name.localeCompare(right.name, undefined, { numeric: true })
	);
}

/**
 * Builds the Obsidian-style file tree from one `files_list` response.
 * Folder ancestors are created on demand so deep files group correctly,
 * folders sort before files, and siblings sort by name.
 */
export function buildWorkspaceTree(
	entries: WorkspaceEntry[],
): WorkspaceTreeNode[] {
	const folders = new Map<string, WorkspaceTreeNode>([
		[
			'',
			{
				name: '',
				relativePath: '',
				kind: 'folder',
				objectId: null,
				objectType: null,
				parseStatus: null,
				children: [],
			},
		],
	]);
	const ensureFolder = (path: string): WorkspaceTreeNode => {
		if (path === '') return folders.get('')!;
		const existing = folders.get(path);
		if (existing) return existing;
		const parentNode = ensureFolder(parentPathOf(path) ?? '');
		const node: WorkspaceTreeNode = {
			name: path.slice(path.lastIndexOf('/') + 1),
			relativePath: path,
			kind: 'folder',
			objectId: null,
			objectType: null,
			parseStatus: null,
			children: [],
		};
		folders.set(path, node);
		parentNode.children.push(node);
		return node;
	};
	for (const entry of entries) {
		ensureFolder(
			entry.kind === 'folder'
				? entry.relativePath
				: (parentPathOf(entry.relativePath) ?? ''),
		);
	}
	for (const entry of entries) {
		if (entry.kind === 'folder') continue;
		ensureFolder(parentPathOf(entry.relativePath) ?? '').children.push({
			name: entry.name,
			relativePath: entry.relativePath,
			kind: 'file',
			objectId: entry.objectId,
			objectType: entry.objectType,
			parseStatus: entry.parseStatus,
			children: [],
		});
	}
	for (const folder of folders.values()) {
		folder.children.sort(compareNodes);
	}
	return [...folders.get('')!.children];
}

function parentPathOf(path: string): string | null {
	const cut = path.lastIndexOf('/');
	return cut === -1 ? null : path.slice(0, cut);
}

/**
 * First free path for a new document inside one folder: `untitled.md`,
 * then `untitled-2.md`, and so on. Pure so creation actions can stay
 * deterministic without a round trip.
 */
export function nextUntitledPath(
	existingPaths: ReadonlySet<string>,
	folder: string,
	stem = 'untitled',
	extension = 'md',
): string {
	const prefix = folder.length > 0 ? `${folder}/` : '';
	for (let index = 1; index < 10_000; index += 1) {
		const candidate = `${prefix}${stem}${index === 1 ? '' : `-${index}`}.${extension}`;
		if (!existingPaths.has(candidate)) return candidate;
	}
	return `${prefix}${stem}-${Date.now()}.${extension}`;
}

/** Collect every file path in a tree — the disambiguation input. */
export function collectFilePaths(nodes: WorkspaceTreeNode[]): Set<string> {
	const paths = new Set<string>();
	const walk = (list: WorkspaceTreeNode[]) => {
		for (const node of list) {
			if (node.kind === 'file') paths.add(node.relativePath);
			walk(node.children);
		}
	};
	walk(nodes);
	return paths;
}

/** Collect the direct child folder names of one folder (or workspace root). */
export function collectFolderNames(
	nodes: WorkspaceTreeNode[],
	folder: string,
): Set<string> {
	const target = folder.length > 0 ? `${folder}/` : '';
	const names = new Set<string>();
	const walk = (list: WorkspaceTreeNode[]) => {
		for (const node of list) {
			if (node.kind === 'folder') {
				if (
					node.relativePath.startsWith(target) &&
					!node.relativePath.slice(target.length).includes('/')
				) {
					names.add(node.name);
				}
				walk(node.children);
			}
		}
	};
	walk(nodes);
	return names;
}

export interface TreeNavigationTarget {
	route: string | null;
	/** Query parameters for the route, e.g. {selected: id} or {raw: path}. */
	query: Record<string, string>;
}

/**
 * Where a tree row opens. Managed objects route by type to their domain
 * page; unmanaged and malformed Markdown open the source-backed editor;
 * folders and opaque binaries are not openable here.
 */
export function treeTargetFor(node: WorkspaceTreeNode): TreeNavigationTarget {
	if (node.kind !== 'file') return { route: null, query: {} };
	if (node.parseStatus === 'managed') {
		const route = node.objectType ? ROUTES_BY_TYPE[node.objectType] : undefined;
		if (route && node.objectId) {
			return { route, query: { selected: node.objectId } };
		}
		return { route: null, query: {} };
	}
	if (node.parseStatus === 'unmanaged' || node.parseStatus === 'malformed') {
		return { route: '/notes', query: { raw: node.relativePath } };
	}
	return { route: null, query: {} };
}
