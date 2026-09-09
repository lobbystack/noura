/**
 * Per-module sidebar registry.
 *
 * Each module decides which sidebar content its routes get — or nothing at
 * all. Contributions are gated by the module's plugin being enabled, so
 * turning a module off removes its sidebar the same way its rail entry
 * disappears. This data-driven registry is the interim seam while plugins
 * cannot contribute their own UI components (see docs/architecture/
 * plugin-runtime.md); a future views.register capability can replace these
 * hardcoded entries without changing the renderer.
 */
export interface SidebarModule {
	id: string;
	/** Plugin whose enabled state gates this contribution; null = always. */
	pluginId: string | null;
	/** Route pathnames (prefix-matched) where the section appears. */
	routes: readonly string[];
}

export const SIDEBAR_MODULES: readonly SidebarModule[] = [
	{ id: 'projects', pluginId: 'projects', routes: ['/projects'] },
	{ id: 'tasks-views', pluginId: 'tasks', routes: ['/tasks'] },
	{ id: 'file-browser', pluginId: 'folders', routes: ['/notes', '/pdf'] },
];

function routeMatches(pathname: string, route: string): boolean {
	return pathname === route || pathname.startsWith(`${route}/`);
}

/**
 * The sidebar module for one route, or null when the route gets no sidebar
 * at all. The module with the longer route prefix wins on shared prefixes;
 * the first registered module wins ties.
 */
export function sidebarModuleFor(
	pathname: string,
	enabledPluginIds: ReadonlySet<string>,
): SidebarModule | null {
	let best: SidebarModule | null = null;
	for (const module of SIDEBAR_MODULES) {
		if (module.pluginId && !enabledPluginIds.has(module.pluginId)) continue;
		if (!module.routes.some((route) => routeMatches(pathname, route))) continue;
		if (
			best === null ||
			Math.max(...best.routes.map((route) => route.length)) <
				Math.max(...module.routes.map((route) => route.length))
		) {
			best = module;
		}
	}
	return best;
}
