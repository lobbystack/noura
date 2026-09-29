/**
 * The one map from app routes to the plugin that owns them, used by the
 * shell, the rail, the sidebar, the command palette and keyboard shortcuts
 * on every platform. Routes with no plugin (Home, Files, PDFs, Settings) are
 * part of the core app and always available.
 */
export const PLUGIN_ROUTES = [
	{ pluginId: 'ai', route: '/ai' },
	{ pluginId: 'tasks', route: '/tasks' },
	{ pluginId: 'calendar', route: '/calendar' },
	{ pluginId: 'projects', route: '/projects' },
] as const satisfies ReadonlyArray<{ pluginId: string; route: string }>;

export function routeMatches(pathname: string, route: string): boolean {
	return pathname === route || pathname.startsWith(`${route}/`);
}

/** The plugin that gates `pathname`, or null for a core route. */
export function routePlugin(pathname: string): string | null {
	return (
		PLUGIN_ROUTES.find(({ route }) => routeMatches(pathname, route))
			?.pluginId ?? null
	);
}
