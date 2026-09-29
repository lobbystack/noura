import { isPlainTextPath } from './editor/text-files';
import type { WorkspaceEntry } from '@noura/workspace';

export interface WorkspaceTreeNode {
	name: string;
	relativePath: string;
	kind: WorkspaceEntry['kind'];
	objectId: string | null;
	objectType: string | null;
	parseStatus: WorkspaceEntry['parseStatus'];
	/** The file is a cloud placeholder (such as iCloud) not on this device yet. */
	notDownloaded: boolean;
	children: WorkspaceTreeNode[];
}

const ROUTES_BY_TYPE: Record<string, string> = {
	note: '/files',
	task: '/tasks',
	project: '/projects',
};

// One collator for every comparison: `localeCompare` with options builds a
// new one per call, which dominates sorting a large vault.
const collator = new Intl.Collator(undefined, {
	numeric: true,
	sensitivity: 'base',
});

function compareNodes(
	left: WorkspaceTreeNode,
	right: WorkspaceTreeNode,
): number {
	const kindRank = (node: WorkspaceTreeNode) =>
		node.kind === 'folder' ? 0 : 1;
	return (
		kindRank(left) - kindRank(right) ||
		collator.compare(left.name, right.name) ||
		(left.name < right.name ? -1 : left.name > right.name ? 1 : 0)
	);
}

function folderNode(path: string): WorkspaceTreeNode {
	return {
		name: baseName(path),
		relativePath: path,
		kind: 'folder',
		objectId: null,
		objectType: null,
		parseStatus: null,
		notDownloaded: false,
		children: [],
	};
}

/**
 * Builds the Obsidian-style file tree from one `files_list` response.
 * Folder ancestors are created on demand so deep files group correctly,
 * folders sort before files, and siblings sort by name.
 */
export function buildWorkspaceTree(
	entries: WorkspaceEntry[],
): WorkspaceTreeNode[] {
	const folders = new Map<string, WorkspaceTreeNode>([['', folderNode('')]]);
	const ensureFolder = (path: string): WorkspaceTreeNode => {
		const existing = folders.get(path);
		if (existing) return existing;
		const parentNode = ensureFolder(parentPathOf(path) ?? '');
		const node = folderNode(path);
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
			notDownloaded: entry.notDownloaded === true,
			children: [],
		});
	}
	for (const folder of folders.values()) {
		folder.children.sort(compareNodes);
	}
	return [...folders.get('')!.children];
}

export function parentPathOf(path: string): string | null {
	const cut = path.lastIndexOf('/');
	return cut === -1 ? null : path.slice(0, cut);
}

export function baseName(path: string): string {
	return path.slice(path.lastIndexOf('/') + 1);
}

function joinPath(folder: string, name: string): string {
	return folder.length > 0 ? `${folder}/${name}` : name;
}

/** Markdown files show without `.md`, like Obsidian; other files show whole. */
export function displayName(node: Pick<WorkspaceTreeNode, 'kind' | 'name'>) {
	return node.kind === 'file' && /\.md$/i.test(node.name)
		? node.name.slice(0, -3)
		: node.name;
}

/**
 * First free path for a new item inside one folder: `Untitled.md`, then
 * `Untitled 1.md`, and so on, the way Obsidian names new notes. Pass an empty
 * extension for folders.
 */
export function nextUntitledPath(
	existingPaths: ReadonlySet<string>,
	folder: string,
	stem = 'Untitled',
	extension = 'md',
): string {
	return nextFreePath(
		existingPaths,
		folder,
		stem,
		extension ? `.${extension}` : '',
	);
}

function nextFreePath(
	existingPaths: ReadonlySet<string>,
	folder: string,
	stem: string,
	suffix: string,
): string {
	const taken = new Set([...existingPaths].map((path) => path.toLowerCase()));
	for (let index = 0; index < 10_000; index += 1) {
		const candidate = joinPath(
			folder,
			`${stem}${index === 0 ? '' : ` ${index}`}${suffix}`,
		);
		if (!taken.has(candidate.toLowerCase())) return candidate;
	}
	return joinPath(folder, `${stem} ${Date.now()}${suffix}`);
}

/** Where a duplicate of `path` goes: `Plan 1.md` next to `Plan.md`. */
export function duplicatePath(
	existingPaths: ReadonlySet<string>,
	path: string,
): string {
	const name = baseName(path);
	const dot = name.lastIndexOf('.');
	const [stem, suffix] =
		dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ''];
	return nextFreePath(
		new Set([...existingPaths, path]),
		parentPathOf(path) ?? '',
		stem,
		suffix,
	);
}

const INVALID_NAME_CHARACTERS = /[\\/:*?"<>|]/;

/**
 * Why a new file or folder name can't be used, or null when it can. The
 * characters are the ones Windows, macOS and Obsidian refuse, so a vault
 * stays portable.
 */
export function nameProblem(name: string): string | null {
	if (name.trim().length === 0) return 'Enter a name.';
	if (INVALID_NAME_CHARACTERS.test(name))
		return 'Names can’t contain \\ / : * ? " < > or |.';
	if (name.startsWith('.')) return 'Names can’t start with a dot.';
	if (name !== name.trim()) return 'Names can’t start or end with a space.';
	return null;
}

/**
 * The path a rename produces. Markdown files are renamed without `.md`, so
 * the extension is added back; other files and folders use the name as typed.
 */
export function renamedPath(
	node: Pick<WorkspaceTreeNode, 'kind' | 'name' | 'relativePath'>,
	input: string,
): string {
	const name =
		node.kind === 'file' && /\.md$/i.test(node.name)
			? `${input}${node.name.slice(-3)}`
			: input;
	return joinPath(parentPathOf(node.relativePath) ?? '', name);
}

/** The path `path` gets when dropped into `folder` ('' is the root). */
export function movedIntoPath(path: string, folder: string): string {
	return joinPath(folder, baseName(path));
}

/**
 * Whether dropping `path` into `folder` does anything and is allowed: not its
 * current folder, and a folder never into itself or its own descendants.
 */
export function canMoveInto(path: string, folder: string): boolean {
	if ((parentPathOf(path) ?? '') === folder) return false;
	return folder !== path && !folder.startsWith(`${path}/`);
}

/** `path` after `from` was renamed or moved to `to`, or null if unaffected. */
export function remapPath(
	path: string,
	from: string,
	to: string,
): string | null {
	if (path === from) return to;
	if (path.startsWith(`${from}/`)) return `${to}${path.slice(from.length)}`;
	return null;
}

/** The node at `relativePath`, searching only the folders on its path. */
export function findTreeNode(
	nodes: WorkspaceTreeNode[],
	relativePath: string,
): WorkspaceTreeNode | null {
	for (const node of nodes) {
		if (node.relativePath === relativePath) return node;
		if (
			node.kind === 'folder' &&
			relativePath.startsWith(`${node.relativePath}/`)
		)
			return findTreeNode(node.children, relativePath);
	}
	return null;
}

/** What to tell someone who tries to open a file that isn't downloaded. */
export const NOT_DOWNLOADED_MESSAGE =
	'This file isn’t downloaded yet. Try again once it is.';

/** One object change as the tree sees it. */
export interface TreeObjectChange {
	/** `object:created`, `object:updated`, `object:moved` or `object:deleted`. */
	type: string;
	id?: string;
	path?: string;
}

/**
 * Whether object changes leave the tree as it is: each one saves a file the
 * tree already shows for the same object. Anything else needs a new read.
 */
export function objectChangesKeepTree(
	nodes: WorkspaceTreeNode[],
	changes: readonly TreeObjectChange[],
): boolean {
	return (
		changes.length > 0 &&
		changes.every((change) => {
			if (change.type !== 'object:updated' && change.type !== 'object:created')
				return false;
			const node = change.path ? findTreeNode(nodes, change.path) : null;
			return node !== null && node.objectId === change.id;
		})
	);
}

/** The file whose managed object has `objectId`. */
export function findNodeByObjectId(
	nodes: WorkspaceTreeNode[],
	objectId: string,
): WorkspaceTreeNode | null {
	for (const node of nodes) {
		if (node.objectId === objectId) return node;
		const found = findNodeByObjectId(node.children, objectId);
		if (found) return found;
	}
	return null;
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

/** Every file and folder path in a tree. */
export function collectAllPaths(nodes: WorkspaceTreeNode[]): Set<string> {
	const paths = new Set<string>();
	const walk = (list: WorkspaceTreeNode[]) => {
		for (const node of list) {
			paths.add(node.relativePath);
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

/** Every folder path in a tree. */
export function collectFolderPaths(nodes: WorkspaceTreeNode[]): Set<string> {
	const paths = new Set<string>();
	const walk = (list: WorkspaceTreeNode[]) => {
		for (const node of list) {
			if (node.kind === 'folder') {
				paths.add(node.relativePath);
				walk(node.children);
			}
		}
	};
	walk(nodes);
	return paths;
}

/** The folders that must be open for `path` to be visible. */
export function ancestorFolders(path: string): string[] {
	const parts = path.split('/');
	return parts
		.slice(0, -1)
		.map((_, index) => parts.slice(0, index + 1).join('/'));
}

export interface VisibleTreeRow {
	node: WorkspaceTreeNode;
	depth: number;
	/** 1-based position among siblings, for aria-posinset. */
	position: number;
	siblingCount: number;
}

/** The rows a tree shows with `expanded` folders open, in display order. */
export function visibleTreeRows(
	nodes: WorkspaceTreeNode[],
	expanded: ReadonlySet<string>,
): VisibleTreeRow[] {
	const rows: VisibleTreeRow[] = [];
	const walk = (list: WorkspaceTreeNode[], depth: number) => {
		list.forEach((node, index) => {
			rows.push({
				node,
				depth,
				position: index + 1,
				siblingCount: list.length,
			});
			if (node.kind === 'folder' && expanded.has(node.relativePath))
				walk(node.children, depth + 1);
		});
	};
	walk(nodes, 0);
	return rows;
}

/**
 * Type-ahead: the next row after `fromIndex` (wrapping) whose shown name
 * starts with `prefix`, ignoring case. -1 when nothing matches.
 */
export function typeAheadIndex(
	rows: readonly VisibleTreeRow[],
	fromIndex: number,
	prefix: string,
): number {
	if (rows.length === 0 || prefix.length === 0) return -1;
	const wanted = prefix.toLocaleLowerCase();
	// A repeated single letter cycles; a longer prefix may match the current row.
	const start =
		prefix.length > 1 ? Math.max(fromIndex, 0) : Math.max(fromIndex, -1) + 1;
	for (let offset = 0; offset < rows.length; offset += 1) {
		const index = (start + offset) % rows.length;
		if (displayName(rows[index]!.node).toLocaleLowerCase().startsWith(wanted))
			return index;
	}
	return -1;
}

/**
 * The tree after `from` moved to `to`, keeping every other node as is. Returns
 * null when the move can't be applied locally (a missing source or
 * destination folder), so the caller reads the tree again instead.
 */
export function moveTreePath(
	nodes: WorkspaceTreeNode[],
	from: string,
	to: string,
): WorkspaceTreeNode[] | null {
	const moving = findTreeNode(nodes, from);
	if (!moving || from === to) return moving ? nodes : null;
	const destinationFolder = parentPathOf(to) ?? '';
	if (destinationFolder !== '' && !findTreeNode(nodes, destinationFolder))
		return null;
	if (findTreeNode(nodes, to) && from.toLowerCase() !== to.toLowerCase())
		return null;
	const rebase = (node: WorkspaceTreeNode): WorkspaceTreeNode => {
		const relativePath = remapPath(node.relativePath, from, to)!;
		return {
			...node,
			name: baseName(relativePath),
			relativePath,
			children: node.children.map(rebase),
		};
	};
	const moved = rebase(moving);
	const without = removeTreePath(nodes, from);
	const insert = (
		list: WorkspaceTreeNode[],
		folder: string,
	): WorkspaceTreeNode[] => {
		if (folder === destinationFolder)
			return [...list, moved].sort(compareNodes);
		return list.map((node) =>
			node.kind === 'folder' &&
			(destinationFolder === node.relativePath ||
				destinationFolder.startsWith(`${node.relativePath}/`))
				? { ...node, children: insert(node.children, node.relativePath) }
				: node,
		);
	};
	return insert(without, '');
}

/** The tree without the file or folder at `path`. */
export function removeTreePath(
	nodes: WorkspaceTreeNode[],
	path: string,
): WorkspaceTreeNode[] {
	const next: WorkspaceTreeNode[] = [];
	for (const node of nodes) {
		if (node.relativePath === path) continue;
		if (node.kind === 'folder' && path.startsWith(`${node.relativePath}/`)) {
			next.push({ ...node, children: removeTreePath(node.children, path) });
		} else {
			next.push(node);
		}
	}
	return next;
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
	if (/\.pdf$/i.test(node.relativePath))
		return { route: '/pdf', query: { path: node.relativePath } };
	if (node.parseStatus === 'managed') {
		const route = node.objectType ? ROUTES_BY_TYPE[node.objectType] : undefined;
		if (route && node.objectId) {
			return { route, query: { selected: node.objectId } };
		}
		return { route: null, query: {} };
	}
	if (node.parseStatus === 'unmanaged' || node.parseStatus === 'malformed') {
		return { route: '/files', query: { raw: node.relativePath } };
	}
	if (isPlainTextPath(node.relativePath))
		return { route: '/files', query: { raw: node.relativePath } };
	return { route: null, query: {} };
}

export function treeTargetHref(target: TreeNavigationTarget): string | null {
	if (!target.route) return null;
	return `${target.route}?${new URLSearchParams(target.query).toString()}`;
}

/**
 * The tree path of the document a URL shows, so the tree can reveal it. Reads
 * `raw` and `path` directly and resolves `selected` object IDs through the
 * tree.
 */
export function openDocumentPath(
	url: URL,
	nodes: WorkspaceTreeNode[],
): string | null {
	const raw = url.searchParams.get('raw');
	if (url.pathname.startsWith('/files') && raw) return raw;
	const pdf = url.searchParams.get('path');
	if (url.pathname.startsWith('/pdf') && pdf) return pdf;
	const selected = url.searchParams.get('selected');
	if (selected)
		return findNodeByObjectId(nodes, selected)?.relativePath ?? null;
	return null;
}
