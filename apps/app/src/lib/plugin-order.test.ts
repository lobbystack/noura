import { describe, expect, test } from 'bun:test';
import {
	PLUGIN_IDS,
	SIDEBAR_PLUGIN_IDS,
	movePlugin,
	normalizePluginOrder,
} from './plugin-order';

describe('plugin order', () => {
	test('ignores stale entries and restores every first-party plugin', () => {
		expect(
			normalizePluginOrder(['projects', 'notes', 'projects', 'retired-plugin']),
		).toEqual([
			'inbox',
			'projects',
			'notes',
			'ai',
			'tasks',
			'calendar',
			'folders',
			'sync',
		]);
	});

	test('moves a plugin before the drop target', () => {
		expect(movePlugin(PLUGIN_IDS, 'projects', 'notes', false)).toEqual([
			'inbox',
			'ai',
			'projects',
			'notes',
			'tasks',
			'calendar',
			'folders',
			'sync',
		]);
	});

	test('moves a plugin after the drop target', () => {
		expect(movePlugin(PLUGIN_IDS, 'ai', 'projects', true)).toEqual([
			'inbox',
			'notes',
			'tasks',
			'calendar',
			'projects',
			'ai',
			'folders',
			'sync',
		]);
	});

	test('sync is a plugin without a sidebar entry', () => {
		expect(PLUGIN_IDS).toContain('sync');
		expect(SIDEBAR_PLUGIN_IDS as readonly string[]).not.toContain('sync');
	});

	test('keeps Inbox in its saved position', () => {
		const moved = movePlugin(PLUGIN_IDS, 'inbox', 'tasks', true);
		expect(moved).toEqual([
			'ai',
			'notes',
			'tasks',
			'inbox',
			'calendar',
			'projects',
			'folders',
			'sync',
		]);
		expect(normalizePluginOrder(moved)).toEqual(moved);
	});
});
