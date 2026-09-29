const STORAGE_PREFIX = 'noura.last-route.v1:';

/** Workspace pages worth reopening on the next launch. */
const RESTORABLE_ROUTES = [
	'/inbox',
	'/files',
	'/pdf',
	'/tasks',
	'/projects',
	'/calendar',
	'/ai',
];

/** The path and query to reopen for `url`, or null for pages not worth it. */
export function restorableRoute(url: URL): string | null {
	const matches = RESTORABLE_ROUTES.some(
		(route) => url.pathname === route || url.pathname.startsWith(`${route}/`),
	);
	return matches ? `${url.pathname}${url.search}` : null;
}

export interface RouteMemory {
	read(workspaceId: string): string | null;
	write(workspaceId: string, route: string): void;
}

export const localRouteMemory: RouteMemory = {
	read(workspaceId) {
		try {
			const saved = localStorage.getItem(`${STORAGE_PREFIX}${workspaceId}`);
			return saved && saved.startsWith('/') && !saved.startsWith('//')
				? saved
				: null;
		} catch {
			return null;
		}
	},
	write(workspaceId, route) {
		try {
			localStorage.setItem(`${STORAGE_PREFIX}${workspaceId}`, route);
		} catch {
			// Reopening the last page is a convenience; skip it without storage.
		}
	},
};

/**
 * Remembers the page each workspace was on and reopens it once when the
 * workspace opens. The app starts on Home, so the first Home visit for a
 * workspace is replaced by its last page; every later navigation is saved.
 */
export class LastRoute {
	#restored = new Set<string>();

	constructor(private readonly memory: RouteMemory = localRouteMemory) {}

	/** Call after each navigation. Returns a route to go to instead, if any. */
	arrived(workspaceId: string | null | undefined, url: URL): string | null {
		if (!workspaceId) return null;
		// `/` only forwards to Home; wait for where it lands.
		if (url.pathname === '/') return null;
		if (!this.#restored.has(workspaceId)) {
			this.#restored.add(workspaceId);
			const saved = this.memory.read(workspaceId);
			if (url.pathname === '/inbox' && saved && saved !== restorableRoute(url))
				return saved;
		}
		const route = restorableRoute(url);
		if (route) this.memory.write(workspaceId, route);
		return null;
	}
}

export const lastRoute = new LastRoute();
