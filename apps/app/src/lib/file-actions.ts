import { goto } from '$app/navigation';
import { page } from '$app/state';
import { toast } from 'svelte-sonner';
import { isCoreError } from '@noura/workspace';
import { flushPendingDrafts } from './editor/pending-drafts.svelte';
import { getNouraClient, workspace } from './state.svelte';
import { tabsStore } from './tabs.svelte';
import {
	baseName,
	canMoveInto,
	collectAllPaths,
	displayName,
	duplicatePath,
	findTreeNode,
	movedIntoPath,
	nameProblem,
	nextUntitledPath,
	parentPathOf,
	remapPath,
	renamedPath,
	treeTargetFor,
	treeTargetHref,
	type WorkspaceTreeNode,
} from './workspace-tree';
import { workspaceTree } from './workspace-tree.svelte';

/**
 * File tree commands shared by the tree, the menu bar and keyboard
 * shortcuts. Each one runs a typed client call, keeps tabs and the open
 * document in step, and reports problems as a toast in plain words.
 */

function problem(error: unknown, fallback: string): string {
	if (isCoreError(error)) {
		if (error.code === 'path_exists')
			return 'Something with that name is already there.';
		if (error.code === 'open_not_allowed') return error.message;
		if (error.code === 'collaboration_transaction_required')
			return 'noura can’t rename, move or delete a file while it’s open for live editing.';
	}
	return fallback;
}

function quoted(node: Pick<WorkspaceTreeNode, 'kind' | 'name'>) {
	return `“${displayName(node)}”`;
}

/** Tab id a tree row's document uses, so a double-click can keep it open. */
function tabObjectId(node: WorkspaceTreeNode): string | null {
	const target = treeTargetFor(node);
	if (!target.route) return null;
	if (target.query.selected) return target.query.selected;
	if (target.query.raw) return `raw:${target.query.raw}`;
	if (target.query.path)
		return `pdf:${workspace.state?.workspaceId}:${target.query.path}`;
	return null;
}

/**
 * Open a file. Files noura shows open in their editor; anything else opens
 * with its default app. `keep` opens a normal tab instead of a preview.
 */
export async function openTreeNode(
	node: WorkspaceTreeNode,
	options: { keep?: boolean } = {},
): Promise<void> {
	if (node.kind !== 'file') return;
	const href = treeTargetHref(treeTargetFor(node));
	if (!href) {
		await openWithDefaultApp(node);
		return;
	}
	const objectId = tabObjectId(node);
	if (options.keep && objectId) tabsStore.keepObject(objectId);
	await goto(href);
}

/** Create an empty note in `folder` and open it. */
export async function createNote(folder = workspaceTree.targetFolder) {
	const path = nextUntitledPath(collectAllPaths(workspaceTree.tree), folder);
	const title = baseName(path).replace(/\.md$/i, '');
	try {
		const result = await getNouraClient().notes.create({
			title,
			relativePath: path,
		});
		workspaceTree.expandTo(folder);
		workspaceTree.selectedPath = path;
		await workspaceTree.refresh();
		if (result.value) {
			tabsStore.keepObject(result.value.id);
			await goto(`/files?selected=${encodeURIComponent(result.value.id)}`);
		}
	} catch (error) {
		toast.error(problem(error, 'Couldn’t create the note.'));
	}
}

/** Create a folder in `folder` and start renaming it, like Obsidian. */
export async function createFolder(folder = workspaceTree.targetFolder) {
	const path = nextUntitledPath(
		collectAllPaths(workspaceTree.tree),
		folder,
		'Untitled',
		'',
	);
	try {
		await getNouraClient().folders.create({ relativePath: path });
		workspaceTree.expandTo(folder);
		await workspaceTree.refresh();
		workspaceTree.selectedPath = path;
		workspaceTree.renamingPath = path;
	} catch (error) {
		toast.error(problem(error, 'Couldn’t create the folder.'));
	}
}

/**
 * Move `from` to `to`: the shared step behind rename and drag and drop.
 * Pending edits are saved first so an open editor never writes to the old
 * path. Returns false when nothing moved.
 */
async function movePath(
	node: WorkspaceTreeNode,
	to: string,
	failure: string,
): Promise<boolean> {
	const from = node.relativePath;
	if (from === to) return false;
	const taken = collectAllPaths(workspaceTree.tree);
	const caseOnly = from.toLowerCase() === to.toLowerCase();
	if (
		!caseOnly &&
		[...taken].some((path) => path.toLowerCase() === to.toLowerCase())
	) {
		toast.error(
			`Something named “${baseName(to)}” is already in ${
				parentPathOf(to) ? `“${parentPathOf(to)}”` : 'the top folder'
			}.`,
		);
		return false;
	}
	if (!(await flushPendingDrafts())) {
		toast.error('Save or discard your changes first, then try again.');
		return false;
	}
	const client = getNouraClient();
	try {
		if (node.kind === 'folder') await client.folders.move({ from, to });
		else await client.files.move({ from, to });
	} catch (error) {
		toast.error(problem(error, failure));
		return false;
	}
	workspaceTree.applyMove(from, to);
	tabsStore.movePath(from, to);
	await followOpenDocument(from, to);
	void workspaceTree.refresh();
	return true;
}

/** Keep the address pointing at the document the user was looking at. */
async function followOpenDocument(from: string, to: string) {
	const url = page.url;
	for (const key of ['raw', 'path']) {
		const current = url.searchParams.get(key);
		const next = current === null ? null : remapPath(current, from, to);
		if (next !== null) {
			const moved = new URL(url);
			moved.searchParams.set(key, next);
			await goto(`${moved.pathname}${moved.search}`, { replaceState: true });
			return;
		}
	}
}

/** Rename a file or folder to `input` (a Markdown file's name without `.md`). */
export async function renameTreeNode(
	node: WorkspaceTreeNode,
	input: string,
): Promise<boolean> {
	const name = input.trim();
	if (name === displayName(node)) return true;
	const issue = nameProblem(name);
	if (issue) {
		toast.error(issue);
		return false;
	}
	const moved = await movePath(
		node,
		renamedPath(node, name),
		`Couldn’t rename ${quoted(node)}.`,
	);
	if (!moved) return false;
	// A note's title is its file name, so a note with an ID keeps its title in
	// step through the normal update path.
	if (node.objectId && node.objectType === 'note') {
		const client = getNouraClient();
		try {
			const note = await client.notes.get(node.objectId);
			if (note.title !== name) {
				await client.notes.update(node.objectId, {
					title: name,
					expectedRevision: note.revision,
				});
			}
			tabsStore.renameObject(node.objectId, name);
		} catch {
			toast.error('noura renamed the file but couldn’t update its title.');
		}
	}
	return true;
}

/** Move a file or folder into `folder` ('' is the top of the workspace). */
export async function moveTreeNode(node: WorkspaceTreeNode, folder: string) {
	if (!canMoveInto(node.relativePath, folder)) return false;
	const moved = await movePath(
		node,
		movedIntoPath(node.relativePath, folder),
		`Couldn’t move ${quoted(node)}.`,
	);
	if (moved) {
		workspaceTree.expandTo(folder);
		workspaceTree.selectedPath = movedIntoPath(node.relativePath, folder);
	}
	return moved;
}

/** Duplicate a file next to itself: `Plan.md` becomes `Plan 1.md`. */
export async function duplicateTreeNode(node: WorkspaceTreeNode) {
	if (node.kind !== 'file') return;
	const to = duplicatePath(
		collectAllPaths(workspaceTree.tree),
		node.relativePath,
	);
	try {
		await getNouraClient().files.copy({ from: node.relativePath, to });
		await workspaceTree.refresh();
		workspaceTree.selectedPath = to;
		const copy = findTreeNode(workspaceTree.tree, to);
		if (copy) await openTreeNode(copy);
	} catch (error) {
		toast.error(problem(error, `Couldn’t duplicate ${quoted(node)}.`));
	}
}

function objectIdsUnder(node: WorkspaceTreeNode): string[] {
	return [
		...(node.objectId ? [node.objectId] : []),
		...node.children.flatMap(objectIdsUnder),
	];
}

/** Move a file or folder to the system trash. */
export async function trashTreeNode(node: WorkspaceTreeNode) {
	const path = node.relativePath;
	const objectIds = new Set(objectIdsUnder(node));
	const url = page.url;
	const showing =
		[url.searchParams.get('raw'), url.searchParams.get('path')].some(
			(value) => value !== null && remapPath(value, path, path) !== null,
		) || objectIds.has(url.searchParams.get('selected') ?? '');
	// Save what is typed so the trashed copy has it; a failed save doesn't
	// stop a deliberate delete.
	await flushPendingDrafts();
	let fallback: string | null;
	try {
		fallback = await getNouraClient().files.trash({ relativePath: path });
	} catch (error) {
		toast.error(problem(error, `Couldn’t move ${quoted(node)} to the trash.`));
		return;
	}
	tabsStore.closePath(path);
	for (const tab of tabsStore.tabs.filter((tab) => objectIds.has(tab.objectId)))
		tabsStore.close(tab.id);
	if (
		workspaceTree.selectedPath &&
		remapPath(workspaceTree.selectedPath, path, path)
	)
		workspaceTree.selectedPath = parentPathOf(path);
	void workspaceTree.refresh();
	toast.success(
		fallback
			? `Moved ${quoted(node)} to .noura/trash in this workspace.`
			: `Moved ${quoted(node)} to the trash.`,
	);
	if (showing) await goto(url.pathname, { replaceState: true });
}

export async function revealTreeNode(node: WorkspaceTreeNode) {
	try {
		await getNouraClient().files.reveal({ relativePath: node.relativePath });
	} catch (error) {
		toast.error(problem(error, `Couldn’t show ${quoted(node)}.`));
	}
}

export async function openWithDefaultApp(node: WorkspaceTreeNode) {
	try {
		await getNouraClient().files.openWithDefaultApp({
			relativePath: node.relativePath,
		});
	} catch (error) {
		toast.error(problem(error, `Couldn’t open ${quoted(node)}.`));
	}
}
