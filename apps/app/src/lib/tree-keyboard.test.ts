import { describe, expect, test } from 'bun:test';
import { treeKeyAction } from './tree-keyboard';
import { buildWorkspaceTree, visibleTreeRows } from './workspace-tree';

function entry(relativePath: string, kind: 'file' | 'folder' = 'file') {
	return {
		relativePath,
		name: relativePath.split('/').at(-1) ?? relativePath,
		kind,
		parseStatus: null,
		objectId: null,
		objectType: null,
		revision: null,
	};
}

const tree = buildWorkspaceTree([
	entry('docs', 'folder'),
	entry('docs/a.md'),
	entry('docs/b.md'),
	entry('readme.md'),
]);
const open = new Set(['docs']);
const rows = visibleTreeRows(tree, open);
const key = (name: string, extra: Record<string, boolean> = {}) => ({
	key: name,
	...extra,
});

describe('tree keyboard', () => {
	test('arrows move and stop at the ends', () => {
		expect(treeKeyAction(key('ArrowDown'), rows, 0, open, null)).toEqual({
			type: 'focus',
			index: 1,
		});
		expect(treeKeyAction(key('ArrowDown'), rows, 3, open, null)).toEqual({
			type: 'focus',
			index: 3,
		});
		expect(treeKeyAction(key('ArrowUp'), rows, 0, open, null)).toEqual({
			type: 'focus',
			index: 0,
		});
		expect(treeKeyAction(key('End'), rows, 0, open, null)).toEqual({
			type: 'focus',
			index: 3,
		});
		expect(treeKeyAction(key('Home'), rows, 2, open, null)).toEqual({
			type: 'focus',
			index: 0,
		});
	});

	test('right opens a folder, then enters it; left closes or goes up', () => {
		const closed = visibleTreeRows(tree, new Set());
		expect(
			treeKeyAction(key('ArrowRight'), closed, 0, new Set(), null),
		).toEqual({ type: 'expand', index: 0 });
		expect(treeKeyAction(key('ArrowRight'), rows, 0, open, null)).toEqual({
			type: 'focus',
			index: 1,
		});
		expect(treeKeyAction(key('ArrowLeft'), rows, 0, open, null)).toEqual({
			type: 'collapse',
			index: 0,
		});
		expect(treeKeyAction(key('ArrowLeft'), rows, 2, open, null)).toEqual({
			type: 'focus',
			index: 0,
		});
		expect(treeKeyAction(key('ArrowLeft'), rows, 3, open, null)).toBeNull();
		expect(treeKeyAction(key('ArrowRight'), rows, 3, open, null)).toBeNull();
	});

	test('file manager keys', () => {
		expect(treeKeyAction(key('Enter'), rows, 1, open, null)).toEqual({
			type: 'activate',
			index: 1,
		});
		expect(treeKeyAction(key('Enter'), rows, 1, open, 'docs/a.md')).toEqual({
			type: 'rename',
			index: 1,
		});
		expect(treeKeyAction(key(' '), rows, 1, open, 'docs/a.md')).toEqual({
			type: 'activate',
			index: 1,
		});
		expect(treeKeyAction(key('F2'), rows, 0, open, null)).toEqual({
			type: 'rename',
			index: 0,
		});
		expect(treeKeyAction(key('Delete'), rows, 0, open, null)).toEqual({
			type: 'trash',
			index: 0,
		});
		expect(treeKeyAction(key('Backspace'), rows, 0, open, null)).toBeNull();
		expect(
			treeKeyAction(key('Backspace', { metaKey: true }), rows, 0, open, null),
		).toEqual({ type: 'trash', index: 0 });
		expect(
			treeKeyAction(key('F10', { shiftKey: true }), rows, 2, open, null),
		).toEqual({ type: 'menu', index: 2 });
		expect(
			treeKeyAction(key('ArrowDown', { metaKey: true }), rows, 1, open, null),
		).toEqual({ type: 'activate', index: 1 });
		expect(treeKeyAction(key('a'), rows, 0, open, null)).toBeNull();
		expect(treeKeyAction(key('ArrowDown'), [], 0, open, null)).toBeNull();
	});
});
