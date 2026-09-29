import { describe, expect, test } from 'bun:test';
import {
	findTreeNode,
	buildWorkspaceTree,
	collectFilePaths,
	collectFolderNames,
	nextUntitledPath,
	duplicatePath,
	displayName,
	nameProblem,
	renamedPath,
	canMoveInto,
	movedIntoPath,
	remapPath,
	visibleTreeRows,
	typeAheadIndex,
	moveTreePath,
	removeTreePath,
	ancestorFolders,
	openDocumentPath,
	findNodeByObjectId,
	objectChangesKeepTree,
	treeTargetFor,
	type WorkspaceTreeNode,
} from './workspace-tree';

function entry(
	relativePath: string,
	kind: 'file' | 'folder' = 'file',
	extra: Partial<{
		objectId: string;
		objectType: string;
		parseStatus: 'managed' | 'unmanaged' | 'malformed';
	}> = {},
) {
	return {
		relativePath,
		name: relativePath.split('/').at(-1) ?? relativePath,
		kind,
		parseStatus: null,
		objectId: null,
		objectType: null,
		revision: null,
		...extra,
	};
}

describe('workspace tree builder', () => {
	test('groups files under folders, folders first, alphabetical', () => {
		const tree = buildWorkspaceTree([
			entry('zebra.md'),
			entry('notes/task-in-note.md'),
			entry('notes', 'folder'),
			entry('projects', 'folder'),
			entry('projects/mobile-app/docs', 'folder'),
			entry('projects/mobile-app/docs/spec.md'),
			entry('apple.md'),
		]);
		expect(tree.map((node) => node.name)).toEqual([
			'notes',
			'projects',
			'apple.md',
			'zebra.md',
		]);
		const projects = tree.find((node) => node.name === 'projects');
		expect(projects?.children[0]?.name).toBe('mobile-app');
		expect(projects?.children[0]?.children[0]?.name).toBe('docs');
		expect(projects?.children[0]?.children[0]?.children[0]?.relativePath).toBe(
			'projects/mobile-app/docs/spec.md',
		);
	});

	test('creates ancestor folders even when only files are listed', () => {
		const tree = buildWorkspaceTree([entry('a/b/c/deep.md', 'file')]);
		expect(tree[0]?.name).toBe('a');
		expect(tree[0]?.kind).toBe('folder');
		expect(tree[0]?.children[0]?.relativePath).toBe('a/b');
	});

	test('numeric sibling names sort naturally', () => {
		const tree = buildWorkspaceTree([
			entry('ch/2.md'),
			entry('ch/10.md'),
			entry('ch/1.md'),
		]);
		expect(
			tree[0]?.children.map((node: WorkspaceTreeNode) => node.name),
		).toEqual(['1.md', '2.md', '10.md']);
	});
});

describe('tree navigation targets', () => {
	test('managed objects route to their domain page with selected', () => {
		const managed = buildWorkspaceTree([
			entry('tasks/x.md', 'file', {
				objectId: 'task_01k',
				objectType: 'task',
				parseStatus: 'managed',
			}),
		])[0]?.children[0];
		expect(treeTargetFor(managed!)).toEqual({
			route: '/tasks',
			query: { selected: 'task_01k' },
		});
	});

	test('unmanaged and malformed markdown open the raw editor', () => {
		const tree = buildWorkspaceTree([
			entry('draft.md', 'file', { parseStatus: 'unmanaged' }),
			entry('broken.md', 'file', { parseStatus: 'malformed' }),
		]);
		const byPath = (path: string) =>
			treeTargetFor(tree.find((node) => node.name === path)!);
		expect(byPath('draft.md')).toEqual({
			route: '/files',
			query: { raw: 'draft.md' },
		});
		expect(byPath('broken.md')).toEqual({
			route: '/files',
			query: { raw: 'broken.md' },
		});
	});

	test('plain text and code files open through the native collaborative surface', () => {
		const tree = buildWorkspaceTree([
			entry('draft.txt'),
			entry('main.rs'),
			entry('component.svelte'),
		]);
		for (const node of tree)
			expect(treeTargetFor(node)).toEqual({
				route: '/files',
				query: { raw: node.relativePath },
			});
	});

	test('folders and binaries are not openable', () => {
		const tree = buildWorkspaceTree([
			entry('assets', 'folder'),
			entry('assets/logo.png'),
		]);
		const folder = tree.find((node) => node.name === 'assets');
		expect(treeTargetFor(folder!)).toEqual({ route: null, query: {} });
		expect(treeTargetFor(folder!.children[0]!)).toEqual({
			route: null,
			query: {},
		});
	});
});

describe('creation path helpers', () => {
	test('nextUntitledPath names new files the way Obsidian does', () => {
		const existing = new Set(['notes/Untitled.md', 'notes/untitled 1.md']);
		expect(nextUntitledPath(existing, 'notes')).toBe('notes/Untitled 2.md');
		expect(nextUntitledPath(new Set(), 'notes')).toBe('notes/Untitled.md');
		expect(nextUntitledPath(new Set(), '')).toBe('Untitled.md');
		expect(nextUntitledPath(new Set(['Untitled']), '', 'Untitled', '')).toBe(
			'Untitled 1',
		);
	});

	test('duplicatePath keeps the extension and skips taken names', () => {
		const existing = new Set(['a/Plan.md', 'a/Plan 1.md', 'scan.pdf']);
		expect(duplicatePath(existing, 'a/Plan.md')).toBe('a/Plan 2.md');
		expect(duplicatePath(existing, 'scan.pdf')).toBe('scan 1.pdf');
		expect(duplicatePath(existing, 'Makefile')).toBe('Makefile 1');
	});

	test('collectFilePaths walks the whole tree', () => {
		const tree = buildWorkspaceTree([
			entry('notes/one.md'),
			entry('notes/nested/two.md'),
			entry('notes', 'folder'),
		]);
		expect(collectFilePaths(tree)).toEqual(
			new Set(['notes/one.md', 'notes/nested/two.md']),
		);
	});

	test('collectFolderNames finds direct children of one folder', () => {
		const tree = buildWorkspaceTree([
			entry('projects/mobile-app', 'folder'),
			entry('projects/mobile-app/docs', 'folder'),
			entry('tasks', 'folder'),
		]);
		expect(collectFolderNames(tree, 'projects')).toEqual(
			new Set(['mobile-app']),
		);
		expect(collectFolderNames(tree, '')).toEqual(
			new Set(['projects', 'tasks']),
		);
	});
});

test('PDF files open in the PDF route even before indexing', () => {
	expect(
		treeTargetFor({
			name: 'Lecture.PDF',
			relativePath: 'course/Lecture.PDF',
			kind: 'file',
			parseStatus: null,
			objectId: null,
			objectType: null,
			children: [],
		}),
	).toEqual({ route: '/pdf', query: { path: 'course/Lecture.PDF' } });
});

describe('finding a tree node', () => {
	const tree = buildWorkspaceTree([
		entry('10 - Personnel', 'folder'),
		entry('10 - Personnel/journal.md', 'file', { parseStatus: 'malformed' }),
		entry('10 - Personnel/sub', 'folder'),
		entry('10 - Personnel/sub/deep.md'),
		entry('10 - Personnel.md'),
	]);

	test('finds files at any depth with their parse status', () => {
		expect(findTreeNode(tree, '10 - Personnel/journal.md')?.parseStatus).toBe(
			'malformed',
		);
		expect(findTreeNode(tree, '10 - Personnel/sub/deep.md')?.name).toBe(
			'deep.md',
		);
		expect(findTreeNode(tree, '10 - Personnel.md')?.kind).toBe('file');
	});

	test('returns null for paths the tree does not hold', () => {
		expect(findTreeNode(tree, '10 - Personnel/missing.md')).toBeNull();
		expect(findTreeNode(tree, 'elsewhere/journal.md')).toBeNull();
	});
});

describe('renaming and moving', () => {
	test('markdown files show and rename without .md', () => {
		const file = {
			kind: 'file' as const,
			name: 'Plan.MD',
			relativePath: 'a/Plan.MD',
		};
		expect(displayName(file)).toBe('Plan');
		expect(renamedPath(file, 'Launch')).toBe('a/Launch.MD');
		const pdf = {
			kind: 'file' as const,
			name: 'scan.pdf',
			relativePath: 'scan.pdf',
		};
		expect(displayName(pdf)).toBe('scan.pdf');
		expect(renamedPath(pdf, 'paper.pdf')).toBe('paper.pdf');
		const folder = {
			kind: 'folder' as const,
			name: 'x.md',
			relativePath: 'x.md',
		};
		expect(displayName(folder)).toBe('x.md');
		expect(renamedPath(folder, 'y')).toBe('y');
	});

	test('names that would break on another system are refused', () => {
		for (const name of [
			'',
			'  ',
			'a/b',
			'a\\b',
			'what?',
			'a:b',
			'.hidden',
			' pad',
		])
			expect(nameProblem(name)).not.toBeNull();
		for (const name of ['Plan', 'Q3 review (draft)', 'été', 'v1.2'])
			expect(nameProblem(name)).toBeNull();
	});

	test('folders never move into themselves', () => {
		expect(canMoveInto('a', 'a')).toBe(false);
		expect(canMoveInto('a', 'a/b')).toBe(false);
		expect(canMoveInto('a/x.md', 'a')).toBe(false);
		expect(canMoveInto('a/x.md', '')).toBe(true);
		expect(canMoveInto('ab', 'a')).toBe(true);
		expect(movedIntoPath('a/x.md', 'b/c')).toBe('b/c/x.md');
		expect(movedIntoPath('a/x.md', '')).toBe('x.md');
	});

	test('remapPath follows a renamed folder but not a sibling prefix', () => {
		expect(remapPath('a/b.md', 'a', 'z')).toBe('z/b.md');
		expect(remapPath('a', 'a', 'z')).toBe('z');
		expect(remapPath('ab/c.md', 'a', 'z')).toBeNull();
	});

	test('moveTreePath moves a folder with its children and keeps order', () => {
		const tree = buildWorkspaceTree([
			entry('a', 'folder'),
			entry('a/one.md'),
			entry('b', 'folder'),
			entry('b/z.md'),
			entry('root.md'),
		]);
		const moved = moveTreePath(tree, 'a', 'b/a')!;
		expect(moved.map((node) => node.relativePath)).toEqual(['b', 'root.md']);
		const b = moved[0]!;
		expect(b.children.map((node) => node.relativePath)).toEqual([
			'b/a',
			'b/z.md',
		]);
		expect(b.children[0]!.children[0]!.relativePath).toBe('b/a/one.md');
		expect(moveTreePath(tree, 'root.md', 'missing/root.md')).toBeNull();
		expect(moveTreePath(tree, 'nope.md', 'x.md')).toBeNull();
		expect(moveTreePath(tree, 'root.md', 'b/z.md')).toBeNull();
		const renamed = moveTreePath(tree, 'root.md', 'Alpha.md')!;
		expect(renamed.map((node) => node.name)).toEqual(['a', 'b', 'Alpha.md']);
	});

	test('removeTreePath drops nested entries', () => {
		const tree = buildWorkspaceTree([entry('a/b/c.md'), entry('a/d.md')]);
		const next = removeTreePath(tree, 'a/b');
		expect(next[0]!.children.map((node) => node.relativePath)).toEqual([
			'a/d.md',
		]);
	});
});

describe('keyboard navigation', () => {
	const tree = buildWorkspaceTree([
		entry('Alpha', 'folder'),
		entry('Alpha/inner.md'),
		entry('Beta.md'),
		entry('bravo.md'),
		entry('Charlie.md'),
	]);

	test('visible rows follow expanded folders', () => {
		expect(
			visibleTreeRows(tree, new Set()).map((row) => row.node.relativePath),
		).toEqual(['Alpha', 'Beta.md', 'bravo.md', 'Charlie.md']);
		const open = visibleTreeRows(tree, new Set(['Alpha']));
		expect(open.map((row) => [row.node.relativePath, row.depth])).toEqual([
			['Alpha', 0],
			['Alpha/inner.md', 1],
			['Beta.md', 0],
			['bravo.md', 0],
			['Charlie.md', 0],
		]);
		expect(open[2]).toMatchObject({ position: 2, siblingCount: 4 });
	});

	test('type-ahead cycles through matches and wraps', () => {
		const rows = visibleTreeRows(tree, new Set());
		expect(typeAheadIndex(rows, 0, 'b')).toBe(1);
		expect(typeAheadIndex(rows, 1, 'b')).toBe(2);
		expect(typeAheadIndex(rows, 2, 'b')).toBe(1);
		expect(typeAheadIndex(rows, 1, 'br')).toBe(2);
		expect(typeAheadIndex(rows, 1, 'be')).toBe(1);
		expect(typeAheadIndex(rows, 0, 'z')).toBe(-1);
	});

	test('ancestor folders open to reveal a path', () => {
		expect(ancestorFolders('a/b/c.md')).toEqual(['a', 'a/b']);
		expect(ancestorFolders('c.md')).toEqual([]);
	});
});

describe('the open document', () => {
	const tree = buildWorkspaceTree([
		entry('tasks/x.md', 'file', {
			objectId: 'task_01k',
			objectType: 'task',
			parseStatus: 'managed',
		}),
	]);

	test('resolves raw paths, PDFs and selected objects', () => {
		const at = (href: string) =>
			openDocumentPath(new URL(href, 'http://app'), tree);
		expect(at('/files?raw=a%2Fb.md')).toBe('a/b.md');
		expect(at('/pdf?path=x.pdf')).toBe('x.pdf');
		expect(at('/tasks?selected=task_01k')).toBe('tasks/x.md');
		expect(at('/tasks?selected=task_missing')).toBeNull();
		expect(at('/inbox')).toBeNull();
		expect(findNodeByObjectId(tree, 'task_01k')?.name).toBe('x.md');
	});
});

describe('objectChangesKeepTree', () => {
	const tree = buildWorkspaceTree([
		entry('notes', 'folder'),
		entry('notes/a.md', 'file', { objectId: 'note_a' }),
		entry('notes/b.md', 'file', { objectId: 'note_b' }),
	]);

	test('keeps the tree when every change saves a file it shows', () => {
		expect(
			objectChangesKeepTree(tree, [
				{ type: 'object:updated', id: 'note_a', path: 'notes/a.md' },
				{ type: 'object:updated', id: 'note_b', path: 'notes/b.md' },
			]),
		).toBe(true);
	});

	test('reads again for new files, moves, deletions and unknown ids', () => {
		for (const change of [
			{ type: 'object:created', id: 'note_c', path: 'notes/c.md' },
			{ type: 'object:moved', id: 'note_a', path: 'notes/a.md' },
			{ type: 'object:deleted', id: 'note_a', path: 'notes/a.md' },
			{ type: 'object:updated', id: 'note_x', path: 'notes/a.md' },
			{ type: 'object:updated', id: 'note_a' },
		])
			expect(
				objectChangesKeepTree(tree, [
					{ type: 'object:updated', id: 'note_b', path: 'notes/b.md' },
					change,
				]),
			).toBe(false);
		expect(objectChangesKeepTree(tree, [])).toBe(false);
	});
});
