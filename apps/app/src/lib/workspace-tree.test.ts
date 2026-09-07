import { describe, expect, test } from 'bun:test';
import {
	buildWorkspaceTree,
	collectFilePaths,
	collectFolderNames,
	nextUntitledPath,
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
			route: '/notes',
			query: { raw: 'draft.md' },
		});
		expect(byPath('broken.md')).toEqual({
			route: '/notes',
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
				route: '/notes',
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
	test('nextUntitledPath skips occupied names', () => {
		const existing = new Set(['notes/untitled.md', 'notes/untitled-2.md']);
		expect(nextUntitledPath(existing, 'notes')).toBe('notes/untitled-3.md');
		expect(nextUntitledPath(new Set(), 'notes')).toBe('notes/untitled.md');
		expect(nextUntitledPath(new Set(), '')).toBe('untitled.md');
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
