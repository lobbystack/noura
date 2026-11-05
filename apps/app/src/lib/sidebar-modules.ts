/**
 * Per-module sidebar registry.
 *
 * The sidebar is always there and always ends with the file tree. A module
 * can add its own section above the tree on its routes. Contributions are gated by the module's plugin being enabled, so
 * turning a module off removes its sidebar the same way its rail entry
 * disappears. This data-driven registry is the interim seam while plugins
 * cannot contribute their own UI components (see docs/architecture/
 * plugin-runtime.md); a future views.register capability can replace these
 * hardcoded entries without changing the renderer.
 */
import { routeMatches, routePlugin } from './plugin-routes';

export interface SidebarModule {
	id: string;
	/**
	 * Route pathnames (prefix-matched) where the section appears above the
	 * file tree. The plugin that owns the route gates the section.
	 */
	routes: readonly string[];
}

export const SIDEBAR_MODULES: readonly SidebarModule[] = [
	{ id: 'projects', routes: ['/projects'] },
	{ id: 'tasks-views', routes: ['/tasks'] },
];

/**
 * The section a route adds above the file tree, or null when it adds none. The module with the longer route prefix wins on shared prefixes;
 * the first registered module wins ties.
 */
export function sidebarModuleFor(
	pathname: string,
	enabledPluginIds: ReadonlySet<string>,
): SidebarModule | null {
	let best: SidebarModule | null = null;
	for (const module of SIDEBAR_MODULES) {
		const route = module.routes.find((value) => routeMatches(pathname, value));
		if (!route) continue;
		const pluginId = routePlugin(route);
		if (pluginId && !enabledPluginIds.has(pluginId)) continue;
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
