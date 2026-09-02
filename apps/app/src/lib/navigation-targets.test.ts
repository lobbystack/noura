import { describe, expect, test } from 'bun:test';
import {
	navigationHref,
	searchResultIsVisible,
	searchResultTarget,
} from './navigation-targets';
import type { SearchResult } from '@noura/workspace';

function result(extra: Partial<SearchResult>): SearchResult {
	return {
		objectId: null,
		objectType: null,
		relativePath: 'notes/alpha.md',
		title: 'Alpha',
		snippet: 'a rare needle here',
		highlights: [],
		score: 1,
		revision: 'rev-1',
		...extra,
	};
}

describe('search result navigation', () => {
	test('managed objects route to their domain page with selected', () => {
		expect(
			searchResultTarget(result({ objectId: 'task_01k', objectType: 'task' })),
		).toEqual({
			route: '/tasks',
			pluginId: 'tasks',
			query: { selected: 'task_01k' },
		});
		expect(
			searchResultTarget(result({ objectId: 'note_01k', objectType: 'note' })),
		).toEqual({
			route: '/notes',
			pluginId: 'notes',
			query: { selected: 'note_01k' },
		});
		expect(
			searchResultTarget(
				result({ objectId: 'project_01k', objectType: 'project' }),
			),
		).toEqual({
			route: '/projects',
			pluginId: 'projects',
			query: { selected: 'project_01k' },
		});
	});

	test('idless markdown opens the source-backed editor', () => {
		expect(searchResultTarget(result({ relativePath: 'draft.md' }))).toEqual({
			route: '/notes',
			pluginId: 'notes',
			query: { raw: 'draft.md' },
		});
	});

	test('unknown managed types and partial identities are inert', () => {
		expect(
			searchResultTarget(
				result({ objectId: 'widget_01k', objectType: 'widget' }),
			),
		).toBeNull();
		expect(
			searchResultTarget(result({ objectId: 'note_01k', objectType: null })),
		).toBeNull();
	});

	test('results are visible only when their destination plugin is enabled', () => {
		const enabled = new Set(['notes']);
		expect(
			searchResultIsVisible(
				result({ objectId: 'note_01k', objectType: 'note' }),
				enabled,
			),
		).toBe(true);
		expect(
			searchResultIsVisible(
				result({ objectId: 'task_01k', objectType: 'task' }),
				enabled,
			),
		).toBe(false);
		expect(
			searchResultIsVisible(result({ relativePath: 'draft.md' }), enabled),
		).toBe(true);
	});

	test('navigationHref encodes query parameters', () => {
		expect(
			navigationHref({
				route: '/notes',
				pluginId: 'notes',
				query: { raw: 'a b.md' },
			}),
		).toBe('/notes?raw=a+b.md');
	});
});
