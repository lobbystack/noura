import { pdfHref } from './pdf/navigation';

export type ObjectType = 'note' | 'task' | 'project' | string;

export interface Tab {
	id: string;
	/** A managed object ID, `raw:<path>` for other files, or `pdf:<workspace>:<path>`. */
	objectId: string;
	objectType: ObjectType;
	title: string;
	/**
	 * A preview tab is replaced by the next file you open. It becomes a normal
	 * tab when you edit it or double-click it.
	 */
	preview: boolean;
	href?: string;
	pdfPosition?: { page: number; scale: string };
}

export interface StoredTabs {
	tabs: Tab[];
	activeId: string | null;
}

const ROUTES_BY_TYPE: Record<string, string> = {
	task: '/tasks',
	project: '/projects',
};

/** Where a tab goes when you activate it. */
export function tabHref(tab: Tab): string {
	if (tab.href) return tab.href;
	if (tab.objectId.startsWith('raw:'))
		return `/files?raw=${encodeURIComponent(tab.objectId.slice(4))}`;
	const route = ROUTES_BY_TYPE[tab.objectType] ?? '/files';
	return `${route}?selected=${encodeURIComponent(tab.objectId)}`;
}

/** The workspace file a path-backed tab shows, if it has one. */
export function tabPath(tab: Tab): string | null {
	if (tab.objectId.startsWith('raw:')) return tab.objectId.slice(4);
	if (tab.objectId.startsWith('pdf:')) {
		const rest = tab.objectId.slice(4);
		const cut = rest.indexOf(':');
		return cut === -1 ? null : rest.slice(cut + 1);
	}
	return null;
}

function remapped(path: string, from: string, to: string): string | null {
	if (path === from) return to;
	if (path.startsWith(`${from}/`)) return `${to}${path.slice(from.length)}`;
	return null;
}

function titleFor(path: string, markdownWithoutExtension: boolean) {
	const name = path.slice(path.lastIndexOf('/') + 1);
	return markdownWithoutExtension ? name.replace(/\.md$/i, '') : name;
}

/**
 * Follow a rename or move of `from` (a file or folder) to `to` in every tab
 * that shows a file by path. Tabs for managed objects follow their ID.
 */
export function remapTabPaths(tabs: Tab[], from: string, to: string): Tab[] {
	return tabs.map((tab) => {
		const path = tabPath(tab);
		const next = path === null ? null : remapped(path, from, to);
		if (next === null) return tab;
		if (tab.objectId.startsWith('raw:')) {
			return {
				...tab,
				objectId: `raw:${next}`,
				title: titleFor(next, true),
				href: undefined,
			};
		}
		const prefix = tab.objectId.slice(0, tab.objectId.length - path!.length);
		return {
			...tab,
			objectId: `${prefix}${next}`,
			title: titleFor(next, false),
			href: pdfHref(next),
		};
	});
}

/** Tabs whose file is `path` or inside the folder `path`. */
export function tabsUnderPath(tabs: Tab[], path: string): Tab[] {
	return tabs.filter((tab) => {
		const tabFile = tabPath(tab);
		return tabFile !== null && remapped(tabFile, path, path) !== null;
	});
}

function isTab(value: unknown): value is Tab {
	if (!value || typeof value !== 'object') return false;
	const tab = value as Record<string, unknown>;
	return (
		typeof tab.id === 'string' &&
		typeof tab.objectId === 'string' &&
		typeof tab.objectType === 'string' &&
		typeof tab.title === 'string' &&
		typeof tab.preview === 'boolean' &&
		(tab.href === undefined || typeof tab.href === 'string')
	);
}

/** Read tabs saved by an earlier session, dropping anything malformed. */
export function parseStoredTabs(raw: string | null): StoredTabs {
	if (!raw) return { tabs: [], activeId: null };
	try {
		const value = JSON.parse(raw) as { tabs?: unknown; activeId?: unknown };
		const tabs = Array.isArray(value.tabs)
			? value.tabs.filter(isTab).map((tab) => ({
					id: tab.id,
					objectId: tab.objectId,
					objectType: tab.objectType,
					title: tab.title,
					preview: tab.preview,
					...(tab.href && tab.href.startsWith('/') ? { href: tab.href } : {}),
					...(tab.pdfPosition ? { pdfPosition: tab.pdfPosition } : {}),
				}))
			: [];
		const activeId =
			typeof value.activeId === 'string' &&
			tabs.some((tab) => tab.id === value.activeId)
				? value.activeId
				: (tabs[0]?.id ?? null);
		return { tabs, activeId };
	} catch {
		return { tabs: [], activeId: null };
	}
}
