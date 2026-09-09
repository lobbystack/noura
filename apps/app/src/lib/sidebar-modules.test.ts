import { describe, expect, test } from 'bun:test';
import { SIDEBAR_MODULES, sidebarModuleFor } from './sidebar-modules';

const enabled = new Set(['tasks', 'folders', 'notes', 'calendar', 'projects']);

describe('sidebar modules registry', () => {
	test('declares the bundled contributions', () => {
		expect(SIDEBAR_MODULES.map((module) => module.id)).toEqual([
			'projects',
			'tasks-views',
			'file-browser',
		]);
	});

	test('routes with a contribution resolve to its module', () => {
		expect(sidebarModuleFor('/projects', enabled)?.id).toBe('projects');
		expect(sidebarModuleFor('/tasks', enabled)?.id).toBe('tasks-views');
		expect(sidebarModuleFor('/notes', enabled)?.id).toBe('file-browser');
		expect(sidebarModuleFor('/notes', enabled)?.pluginId).toBe('folders');
	});

	test('routes without contributions get no sidebar at all', () => {
		for (const path of ['/calendar', '/inbox', '/search', '/settings', '/ai']) {
			expect(sidebarModuleFor(path, enabled)).toBeNull();
		}
	});

	test('contributions vanish when their plugin is disabled', () => {
		expect(
			sidebarModuleFor('/projects', new Set(['tasks', 'folders'])),
		).toBeNull();
		const withoutTasks = new Set([...enabled].filter((id) => id !== 'tasks'));
		expect(sidebarModuleFor('/tasks', withoutTasks)).toBeNull();
		const withoutFolders = new Set(
			[...enabled].filter((id) => id !== 'folders'),
		);
		expect(sidebarModuleFor('/notes', withoutFolders)).toBeNull();
	});

	test('deep paths match their route prefix', () => {
		expect(sidebarModuleFor('/tasks', enabled)?.id).toBe('tasks-views');
	});
});
