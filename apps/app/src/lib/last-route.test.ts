import { expect, test } from 'bun:test';
import { LastRoute, restorableRoute, type RouteMemory } from './last-route';

function memory(initial: Record<string, string> = {}) {
	const saved = new Map(Object.entries(initial));
	const store: RouteMemory = {
		read: (id) => saved.get(id) ?? null,
		write: (id, route) => void saved.set(id, route),
	};
	return { saved, store };
}

const at = (href: string) => new URL(href, 'http://app');

test('only workspace pages are remembered', () => {
	expect(restorableRoute(at('/files?raw=a.md'))).toBe('/files?raw=a.md');
	expect(restorableRoute(at('/tasks'))).toBe('/tasks');
	expect(restorableRoute(at('/settings'))).toBeNull();
	expect(restorableRoute(at('/filesystem'))).toBeNull();
	expect(restorableRoute(at('/'))).toBeNull();
});

test('the first Home visit reopens the last page, once per workspace', () => {
	const { saved, store } = memory({ ws: '/files?raw=a.md' });
	const routes = new LastRoute(store);
	expect(routes.arrived('ws', at('/'))).toBeNull();
	expect(routes.arrived('ws', at('/inbox'))).toBe('/files?raw=a.md');
	expect(routes.arrived('ws', at('/files?raw=a.md'))).toBeNull();
	expect(routes.arrived('ws', at('/inbox'))).toBeNull();
	expect(saved.get('ws')).toBe('/inbox');
	expect(routes.arrived('other', at('/inbox'))).toBeNull();
	expect(saved.get('other')).toBe('/inbox');
});

test('a deep link at launch wins over the saved page', () => {
	const { saved, store } = memory({ ws: '/tasks' });
	const routes = new LastRoute(store);
	expect(routes.arrived('ws', at('/files?raw=b.md'))).toBeNull();
	expect(saved.get('ws')).toBe('/files?raw=b.md');
	expect(routes.arrived(null, at('/inbox'))).toBeNull();
});
