import { describe, expect, test } from 'bun:test';
import { routePlugin } from './plugin-routes';

describe('plugin routes', () => {
	test('maps each module route and its subpaths to its plugin', () => {
		expect(routePlugin('/tasks')).toBe('tasks');
		expect(routePlugin('/tasks/anything')).toBe('tasks');
		expect(routePlugin('/calendar')).toBe('calendar');
		expect(routePlugin('/projects')).toBe('projects');
		expect(routePlugin('/ai')).toBe('ai');
	});

	test('core routes have no plugin gate', () => {
		for (const path of ['/inbox', '/files', '/pdf', '/settings', '/tasksx'])
			expect(routePlugin(path)).toBeNull();
	});
});
