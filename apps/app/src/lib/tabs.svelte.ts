import { browser } from '$app/environment';
import {
	PAGE_TABS,
	pageRouteFor,
	pageTabObjectId,
	parseStoredTabs,
	remapTabPaths,
	tabsUnderPath,
	type ObjectType,
	type Tab,
} from './tabs';

export type { ObjectType, Tab } from './tabs';
export { tabHref } from './tabs';

const STORAGE_PREFIX = 'noura.tabs.v1:';

/**
 * Open documents as tabs, per workspace. Tabs are a view preference, so they
 * live in this device's browser storage, never in the workspace.
 */
class TabsStore {
	#workspaceId: string | null | undefined;
	tabs = $state<Tab[]>([]);
	activeId = $state<string | null>(null);
	/** The document tab that was on screen before a page tab took over. */
	#lastDocumentId: string | null = null;
	/** Documents to open as normal tabs instead of previews (a double-click). */
	#keepOnOpen = new Set<string>();

	/** Switch to a workspace's tabs, restoring the ones it had last time. */
	setWorkspace(id: string | null | undefined) {
		if (this.#workspaceId === id) return;
		this.#workspaceId = id;
		const stored = id ? parseStoredTabs(this.#read(id)) : null;
		this.tabs = stored?.tabs ?? [];
		this.activeId = stored?.activeId ?? null;
	}

	/**
	 * Show a document. An open tab for it is reused; otherwise it replaces the
	 * current preview tab, or opens as a new preview tab.
	 */
	open(objectId: string, objectType: ObjectType, title: string) {
		const keep = this.#keepOnOpen.delete(objectId);
		const existing = this.tabs.find((t) => t.objectId === objectId);
		if (existing) {
			this.activeId = existing.id;
			if (keep) this.keep(existing.id);
			this.#save();
			return existing.id;
		}
		const previewIndex = keep ? -1 : this.tabs.findIndex((t) => t.preview);
		const tab: Tab = {
			id: crypto.randomUUID(),
			objectId,
			objectType,
			title,
			preview: !keep,
		};
		if (previewIndex >= 0) {
			this.tabs = this.tabs.map((t, i) => (i === previewIndex ? tab : t));
		} else {
			this.tabs = [...this.tabs, tab];
		}
		this.activeId = tab.id;
		this.#save();
		return tab.id;
	}

	/**
	 * Open an app page such as Calendar in a tab of its own that stays open,
	 * or switch to the one already open.
	 */
	openPage(route: string, title: string) {
		const existing = this.tabs.find(
			(tab) => tab.objectId === pageTabObjectId(route),
		);
		if (existing) {
			this.activeId = existing.id;
			this.keep(existing.id);
			this.#save();
			return existing.id;
		}
		return this.#addPage(route, title, route, false);
	}

	/**
	 * Arriving on an app page shows it as a tab: its open tab, or a preview
	 * tab that the next thing you open replaces. The tab remembers where you
	 * are in the page, such as the Tasks view.
	 */
	followPage(url: URL) {
		const route = pageRouteFor(url);
		if (route === null) return;
		const href = `${url.pathname}${url.search}`;
		const tab = this.tabs.find(
			(candidate) => candidate.objectId === pageTabObjectId(route),
		);
		if (!tab) {
			this.#addPage(route, PAGE_TABS[route]!, href, true);
			return;
		}
		if (this.activeId === tab.id && tab.href === href) return;
		this.#rememberDocument();
		this.activeId = tab.id;
		this.tabs = this.tabs.map((candidate) =>
			candidate.id === tab.id ? { ...candidate, href } : candidate,
		);
		this.#save();
	}

	#rememberDocument() {
		const active = this.active;
		if (active && active.objectType !== 'page')
			this.#lastDocumentId = active.id;
	}

	/**
	 * The document to show on Files: the active tab when it is a document,
	 * otherwise the document that was on screen before a page tab.
	 */
	get documentTab(): Tab | undefined {
		const active = this.active;
		if (active && active.objectType !== 'page') return active;
		return (
			this.tabs.find((tab) => tab.id === this.#lastDocumentId) ??
			this.tabs.findLast((tab) => tab.objectType !== 'page')
		);
	}

	#addPage(route: string, title: string, href: string, preview: boolean) {
		this.#rememberDocument();
		const tab: Tab = {
			id: crypto.randomUUID(),
			objectId: pageTabObjectId(route),
			objectType: 'page',
			title,
			preview,
			href,
		};
		const previewIndex = preview
			? this.tabs.findIndex((candidate) => candidate.preview)
			: -1;
		this.tabs =
			previewIndex >= 0
				? this.tabs.map((candidate, index) =>
						index === previewIndex ? tab : candidate,
					)
				: [...this.tabs, tab];
		this.activeId = tab.id;
		this.#save();
		return tab.id;
	}

	/** Turn a preview tab into a normal tab that stays open. */
	keep(id: string | null) {
		if (!id || !this.tabs.some((tab) => tab.id === id && tab.preview)) return;
		this.tabs = this.tabs.map((t) =>
			t.id === id ? { ...t, preview: false } : t,
		);
		this.#save();
	}

	/** Keep the tab showing `objectId`, or open it as a kept tab next time. */
	keepObject(objectId: string) {
		const tab = this.tabs.find((candidate) => candidate.objectId === objectId);
		if (tab) this.keep(tab.id);
		else this.#keepOnOpen.add(objectId);
	}

	close(id: string) {
		const index = this.tabs.findIndex((t) => t.id === id);
		if (index === -1) return;
		const wasActive = this.activeId === id;
		this.tabs = this.tabs.filter((t) => t.id !== id);
		if (wasActive)
			this.activeId =
				this.tabs[Math.min(index, this.tabs.length - 1)]?.id ?? null;
		this.#save();
	}

	/** Close every tab showing `path` or a file inside the folder `path`. */
	closePath(path: string) {
		for (const tab of tabsUnderPath(this.tabs, path)) this.close(tab.id);
	}

	/** Follow a rename or move so open tabs keep showing the same files. */
	movePath(from: string, to: string) {
		this.tabs = remapTabPaths(this.tabs, from, to);
		this.#save();
	}

	/** The tab `offset` places from the active one, wrapping around. */
	neighbor(offset: number): Tab | undefined {
		if (this.tabs.length === 0) return undefined;
		const index = this.tabs.findIndex((tab) => tab.id === this.activeId);
		const next =
			((((index === -1 ? 0 : index) + offset) % this.tabs.length) +
				this.tabs.length) %
			this.tabs.length;
		return this.tabs[next];
	}

	setLocation(id: string, href: string) {
		this.tabs = this.tabs.map((tab) =>
			tab.id === id ? { ...tab, href } : tab,
		);
		this.#save();
	}
	setPdfPosition(id: string, pdfPosition: { page: number; scale: string }) {
		this.tabs = this.tabs.map((tab) =>
			tab.id === id ? { ...tab, pdfPosition } : tab,
		);
		this.#save();
	}
	clear() {
		this.tabs = [];
		this.activeId = null;
		this.#save();
	}

	setActive(id: string | null) {
		this.activeId = id;
		this.#save();
	}

	/** A file moved: its tab follows it to the new identity and name. */
	retarget(objectId: string, nextObjectId: string, title: string) {
		this.tabs = this.tabs.map((tab) =>
			tab.objectId === objectId
				? { ...tab, objectId: nextObjectId, title }
				: tab,
		);
		this.#save();
	}

	renameObject(objectId: string, title: string) {
		this.tabs = this.tabs.map((tab) =>
			tab.objectId === objectId ? { ...tab, title } : tab,
		);
		this.#save();
	}

	get active() {
		return this.tabs.find((t) => t.id === this.activeId);
	}

	#read(id: string): string | null {
		if (!browser) return null;
		try {
			return localStorage.getItem(`${STORAGE_PREFIX}${id}`);
		} catch {
			return null;
		}
	}

	#save() {
		const id = this.#workspaceId;
		if (!browser || !id) return;
		try {
			localStorage.setItem(
				`${STORAGE_PREFIX}${id}`,
				JSON.stringify({ tabs: this.tabs, activeId: this.activeId }),
			);
		} catch {
			// Tabs still work for this session when storage is unavailable.
		}
	}
}

export const tabsStore = new TabsStore();
