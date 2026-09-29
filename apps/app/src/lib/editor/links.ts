import { goto } from '$app/navigation';
import { toast } from 'svelte-sonner';
import type {
	LinkOpenOptions,
	LinkSuggestion,
	LiveMarkdownOptions,
} from '@noura/editor/types';
import { getNouraClient } from '$lib/state.svelte';
import { markdownAssets } from '$lib/pdf/markdown';
import { tabsStore } from '$lib/tabs.svelte';
import { workspaceTree } from '$lib/workspace-tree.svelte';
import {
	collectFilePaths,
	findTreeNode,
	treeTargetFor,
	type WorkspaceTreeNode,
} from '$lib/workspace-tree';
import {
	linkPath,
	linkSuggestions,
	relativeTarget,
	resolveLinkPath,
} from './link-resolution';

let cachedTree: WorkspaceTreeNode[] | null = null;
let cachedFiles: string[] = [];
let cachedSuggestions: LinkSuggestion[] | null = null;

/** Every file path, from the tree the sidebar already holds. */
function workspaceFiles(): string[] {
	const tree = workspaceTree.tree;
	if (tree !== cachedTree) {
		cachedTree = tree;
		cachedFiles = [...collectFilePaths(tree)];
		cachedSuggestions = null;
	}
	return cachedFiles;
}

function suggestions(): LinkSuggestion[] {
	const files = workspaceFiles();
	cachedSuggestions ??= linkSuggestions(files);
	return cachedSuggestions;
}

const WEB_LINK = /^(?:https?:\/\/|www\.)/i;

function openWebLink(target: string) {
	const url = /^www\./i.test(target) ? `https://${target}` : target;
	void getNouraClient()
		.files.openPdfLink(url)
		.catch(() => toast.error('Couldn’t open that link.'));
}

async function openWorkspaceFile(path: string, options: LinkOpenOptions) {
	const node = findTreeNode(workspaceTree.tree, path);
	const target = node ? treeTargetFor(node) : null;
	if (!node || !target?.route) {
		toast.error('noura can’t open this kind of file yet.');
		return;
	}
	const query = new URLSearchParams(target.query);
	if (options.newTab && node.kind === 'file') {
		// Open as a normal tab next to the current one instead of replacing the
		// preview tab.
		tabsStore.keepObject(
			node.parseStatus === 'managed' && node.objectId
				? node.objectId
				: `raw:${node.relativePath}`,
		);
	}
	await goto(`${target.route}?${query}`);
}

/**
 * Link behaviour for editors showing `sourceRelativePath`: links resolve by
 * path or by file name anywhere in the workspace, like Obsidian; clicking
 * one opens it in the app, web links open in the browser, and `[[`
 * suggests files.
 */
export function editorLinks(
	sourceRelativePath?: string,
): Pick<
	LiveMarkdownOptions,
	| 'resolveImage'
	| 'loadRemoteImage'
	| 'resolveLink'
	| 'openLink'
	| 'linkSuggestions'
	| 'openPdf'
	| 'mountPdfEmbed'
> {
	if (!sourceRelativePath) return {};
	const assets = markdownAssets(sourceRelativePath);
	const locate = (target: string) => {
		const path = resolveLinkPath(sourceRelativePath, target, workspaceFiles());
		if (!path) return target;
		const { fragment } = linkPath(target);
		const relative = relativeTarget(sourceRelativePath, path);
		return fragment ? `${relative}#${fragment}` : relative;
	};
	return {
		...assets,
		resolveImage: (src) =>
			WEB_LINK.test(src) ? src : (assets.resolveImage?.(locate(src)) ?? null),
		loadRemoteImage: async (url) =>
			(await getNouraClient().files.fetchRemoteImage({ url })).dataUrl,
		resolveLink: (target) =>
			assets.resolveLink?.(locate(target)) ??
			Promise.resolve({ kind: 'unresolved' as const }),
		linkSuggestions: suggestions,
		openLink: (target, options) => {
			if (options.kind === 'url' || WEB_LINK.test(target)) {
				if (WEB_LINK.test(target)) openWebLink(target);
				return;
			}
			if (/^[a-z][a-z\d+.-]*:/i.test(target)) return;
			const path = resolveLinkPath(
				sourceRelativePath,
				target,
				workspaceFiles(),
			);
			if (!path) {
				const name = linkPath(target).path || target;
				toast.error(`“${name}” isn’t in this workspace yet.`);
				return;
			}
			void openWorkspaceFile(path, options);
		},
	};
}
