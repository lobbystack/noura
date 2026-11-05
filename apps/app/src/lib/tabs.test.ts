import { describe, expect, test } from 'bun:test';
import {
	pageRouteFor,
	pageTabObjectId,
	pageTabRoute,
	parseStoredTabs,
	remapTabPaths,
	tabHref,
	tabPath,
	tabsUnderPath,
	type Tab,
} from './tabs';

function tab(extra: Partial<Tab> & Pick<Tab, 'objectId'>): Tab {
	return {
		id: extra.objectId,
		objectType: 'note',
		title: 'Title',
		preview: false,
		...extra,
	};
}

describe('tab locations', () => {
	test('each kind of tab opens its own route', () => {
		expect(tabHref(tab({ objectId: 'note_1' }))).toBe('/files?selected=note_1');
		expect(tabHref(tab({ objectId: 'task_1', objectType: 'task' }))).toBe(
			'/tasks?selected=task_1',
		);
		expect(tabHref(tab({ objectId: 'project_1', objectType: 'project' }))).toBe(
			'/projects?selected=project_1',
		);
		expect(
			tabHref(tab({ objectId: 'raw:a b/c.md', objectType: 'markdown' })),
		).toBe('/files?raw=a%20b%2Fc.md');
		expect(
			tabHref(tab({ objectId: 'pdf:ws:x.pdf', href: '/pdf?path=x.pdf' })),
		).toBe('/pdf?path=x.pdf');
	});

	test('path-backed tabs know their file', () => {
		expect(tabPath(tab({ objectId: 'raw:a/b.md' }))).toBe('a/b.md');
		expect(tabPath(tab({ objectId: 'pdf:ws_1:a/b:c.pdf' }))).toBe('a/b:c.pdf');
		expect(tabPath(tab({ objectId: 'note_1' }))).toBeNull();
	});
});

describe('renames and moves', () => {
	const tabs = [
		tab({ objectId: 'raw:a/one.md', objectType: 'markdown', title: 'one' }),
		tab({
			objectId: 'pdf:ws:a/scan.pdf',
			objectType: 'pdf',
			title: 'scan.pdf',
			href: '/pdf?path=a%2Fscan.pdf',
		}),
		tab({ objectId: 'raw:ab/other.md', objectType: 'markdown' }),
		tab({ objectId: 'note_1' }),
	];

	test('tabs follow a renamed folder', () => {
		const next = remapTabPaths(tabs, 'a', 'z/a');
		expect(next[0]).toMatchObject({ objectId: 'raw:z/a/one.md', title: 'one' });
		expect(next[1]).toMatchObject({
			objectId: 'pdf:ws:z/a/scan.pdf',
			title: 'scan.pdf',
			href: '/pdf?path=z%2Fa%2Fscan.pdf',
		});
		expect(next[2]).toBe(tabs[2]!);
		expect(next[3]).toBe(tabs[3]!);
	});

	test('tabs follow a renamed file', () => {
		const next = remapTabPaths(tabs, 'a/one.md', 'a/Two.md');
		expect(next[0]).toMatchObject({ objectId: 'raw:a/Two.md', title: 'Two' });
	});

	test('trashing a folder finds the tabs inside it', () => {
		expect(tabsUnderPath(tabs, 'a').map((t) => t.objectId)).toEqual([
			'raw:a/one.md',
			'pdf:ws:a/scan.pdf',
		]);
	});
});

describe('saved tabs', () => {
	test('restores valid tabs and drops the rest', () => {
		const stored = parseStoredTabs(
			JSON.stringify({
				tabs: [
					tab({ objectId: 'note_1', id: 't1', preview: true }),
					{ id: 'bad' },
					tab({ objectId: 'raw:x.md', id: 't2', href: 'https://evil' }),
				],
				activeId: 't2',
			}),
		);
		expect(stored.tabs.map((t) => t.id)).toEqual(['t1', 't2']);
		expect(stored.tabs[1]!.href).toBeUndefined();
		expect(stored.activeId).toBe('t2');
	});

	test('falls back to no tabs on broken data', () => {
		expect(parseStoredTabs('{')).toEqual({ tabs: [], activeId: null });
		expect(parseStoredTabs(null)).toEqual({ tabs: [], activeId: null });
		expect(
			parseStoredTabs(JSON.stringify({ tabs: [], activeId: 'gone' })),
		).toEqual({ tabs: [], activeId: null });
	});
});

describe('page tabs', () => {
	test('a page tab goes to its page and keeps its place across reloads', () => {
		const calendar = tab({
			objectId: pageTabObjectId('/calendar'),
			objectType: 'page',
			title: 'Calendar',
			href: '/calendar?week=2026-09-28',
		});
		expect(pageTabRoute(calendar)).toBe('/calendar');
		expect(tabHref(calendar)).toBe('/calendar?week=2026-09-28');
		expect(tabPath(calendar)).toBeNull();
		const stored = parseStoredTabs(
			JSON.stringify({ tabs: [calendar], activeId: calendar.id }),
		);
		expect(stored.tabs).toEqual([calendar]);
	});

	test('app pages get a tab; documents and Files do not', () => {
		const at = (path: string) => pageRouteFor(new URL(path, 'http://x'));
		expect(at('/inbox')).toBe('/inbox');
		expect(at('/tasks?view=upcoming')).toBe('/tasks');
		expect(at('/calendar')).toBe('/calendar');
		expect(at('/tasks?selected=task_01')).toBeNull();
		expect(at('/files')).toBeNull();
		expect(at('/pdf?path=a.pdf')).toBeNull();
		expect(at('/settings')).toBeNull();
	});

	test('document tabs are not page tabs', () => {
		expect(pageTabRoute(tab({ objectId: 'note_01' }))).toBeNull();
		expect(pageTabRoute(tab({ objectId: 'raw:page:odd.md' }))).toBeNull();
	});
});
