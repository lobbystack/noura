import { parentPathOf, type VisibleTreeRow } from './workspace-tree';

export type TreeKeyAction =
	| { type: 'focus'; index: number }
	| { type: 'expand'; index: number }
	| { type: 'collapse'; index: number }
	| { type: 'activate'; index: number }
	| { type: 'rename'; index: number }
	| { type: 'trash'; index: number }
	| { type: 'menu'; index: number };

export interface TreeKey {
	key: string;
	metaKey?: boolean;
	ctrlKey?: boolean;
	shiftKey?: boolean;
	altKey?: boolean;
}

/**
 * What a key does in the file tree, following the WAI-ARIA tree pattern plus
 * the file manager keys: F2 renames, Delete (or Cmd/Ctrl+Backspace) trashes,
 * and Shift+F10 or the menu key opens the context menu. Enter and Space open
 * a file or toggle a folder; Enter on the file that is already open renames
 * it, like Finder. Returns null for keys the tree leaves alone.
 */
export function treeKeyAction(
	event: TreeKey,
	rows: readonly VisibleTreeRow[],
	index: number,
	expanded: ReadonlySet<string>,
	activePath: string | null,
): TreeKeyAction | null {
	if (rows.length === 0) return null;
	const current = index >= 0 && index < rows.length ? index : 0;
	const row = rows[current]!;
	const node = row.node;
	const isOpenFolder =
		node.kind === 'folder' && expanded.has(node.relativePath);
	const mod = event.metaKey || event.ctrlKey;
	if (event.altKey) return null;
	switch (event.key) {
		case 'ArrowDown':
			if (mod && node.kind === 'file')
				return { type: 'activate', index: current };
			return mod
				? null
				: { type: 'focus', index: Math.min(current + 1, rows.length - 1) };
		case 'ArrowUp':
			return mod ? null : { type: 'focus', index: Math.max(current - 1, 0) };
		case 'Home':
			return { type: 'focus', index: 0 };
		case 'End':
			return { type: 'focus', index: rows.length - 1 };
		case 'ArrowRight':
			if (mod || node.kind !== 'folder') return null;
			if (!isOpenFolder) return { type: 'expand', index: current };
			return rows[current + 1]?.depth === row.depth + 1
				? { type: 'focus', index: current + 1 }
				: null;
		case 'ArrowLeft': {
			if (mod) return null;
			if (isOpenFolder) return { type: 'collapse', index: current };
			const parent = parentPathOf(node.relativePath);
			if (parent === null) return null;
			const parentIndex = rows.findIndex(
				(candidate) => candidate.node.relativePath === parent,
			);
			return parentIndex === -1 ? null : { type: 'focus', index: parentIndex };
		}
		case 'Enter':
			if (mod || event.shiftKey) return null;
			if (node.kind === 'file' && node.relativePath === activePath)
				return { type: 'rename', index: current };
			return { type: 'activate', index: current };
		case ' ':
			return mod ? null : { type: 'activate', index: current };
		case 'F2':
			return { type: 'rename', index: current };
		case 'Delete':
			return { type: 'trash', index: current };
		case 'Backspace':
			return mod ? { type: 'trash', index: current } : null;
		case 'F10':
			return event.shiftKey ? { type: 'menu', index: current } : null;
		case 'ContextMenu':
			return { type: 'menu', index: current };
		default:
			return null;
	}
}
