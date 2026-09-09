export const PLUGIN_IDS = [
	'ai',
	'notes',
	'tasks',
	'calendar',
	'projects',
	'folders',
] as const;

export const SIDEBAR_PLUGIN_IDS = [
	'inbox',
	'ai',
	'notes',
	'tasks',
	'calendar',
	'projects',
] as const;

export type PluginId = (typeof PLUGIN_IDS)[number];
export type NavigationId = PluginId | 'inbox';

function isPluginId(id: string): id is NavigationId {
	return id === 'inbox' || (PLUGIN_IDS as readonly string[]).includes(id);
}

/** Restores a saved order while retaining newly added first-party plugins. */
export function normalizePluginOrder(
	order: readonly string[] | null | undefined = null,
): NavigationId[] {
	const saved = order?.filter(isPluginId) ?? [];
	return [
		...new Set<NavigationId>([
			...(saved.includes('inbox') ? [] : (['inbox'] as const)),
			...saved,
			...PLUGIN_IDS,
		]),
	];
}

/** Moves `pluginId` immediately before or after `targetPluginId`. */
export function movePlugin(
	order: readonly string[],
	pluginId: string,
	targetPluginId: string,
	after: boolean,
): NavigationId[] {
	const normalized = normalizePluginOrder(order);
	if (
		!isPluginId(pluginId) ||
		!isPluginId(targetPluginId) ||
		pluginId === targetPluginId
	) {
		return normalized;
	}

	const next = normalized.filter((id) => id !== pluginId);
	const targetIndex = next.indexOf(targetPluginId);
	next.splice(targetIndex + Number(after), 0, pluginId);
	return next;
}
