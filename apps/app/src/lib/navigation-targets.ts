import type { SearchResult } from '@noura/workspace';

/** The route that opens Markdown and text files. */
export const FILES_ROUTE = '/files';

export interface NavigationTarget {
	route: string;
	pluginId: string;
	query: Record<string, string>;
}

const TARGETS_BY_TYPE: Record<
	string,
	Pick<NavigationTarget, 'route' | 'pluginId'>
> = {
	note: { route: FILES_ROUTE, pluginId: 'notes' },
	task: { route: '/tasks', pluginId: 'tasks' },
	project: { route: '/projects', pluginId: 'projects' },
};

/**
 * Where a full-text search hit opens. Managed objects route to their domain
 * page with a selected parameter; Markdown without a stable ID opens the
 * source-backed editor; anything else (binaries, unknown types) is inert.
 */
export function searchResultTarget(
	result: SearchResult,
): NavigationTarget | null {
	if (result.objectId && result.objectType) {
		const target = TARGETS_BY_TYPE[result.objectType];
		return target ? { ...target, query: { selected: result.objectId } } : null;
	}
	if (!result.objectId && !result.objectType) {
		// Idless Markdown rows (unmanaged or malformed) search as raw content.
		return {
			route: FILES_ROUTE,
			pluginId: 'notes',
			query: { raw: result.relativePath },
		};
	}
	return null;
}

/** Where a workspace object opens, or null for types without a page. */
export function objectHref(
	objectType: string,
	objectId: string,
): string | null {
	const target = TARGETS_BY_TYPE[objectType];
	return target
		? navigationHref({ ...target, query: { selected: objectId } })
		: null;
}

/** Opens a file by its workspace-relative path in the file editor. */
export function fileHref(relativePath: string): string {
	return navigationHref({
		route: FILES_ROUTE,
		pluginId: 'notes',
		query: { raw: relativePath },
	});
}

export function navigationHref(target: NavigationTarget): string {
	const params = new URLSearchParams(target.query);
	return `${target.route}?${params.toString()}`;
}

export function searchResultIsVisible(
	result: SearchResult,
	enabledPluginIds: ReadonlySet<string>,
): boolean {
	const target = searchResultTarget(result);
	return !target || enabledPluginIds.has(target.pluginId);
}
