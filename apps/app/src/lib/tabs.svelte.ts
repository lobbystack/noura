export type ObjectType = 'note' | 'task' | 'project' | string;

export interface Tab {
	id: string;
	objectId: string;
	objectType: ObjectType;
	title: string;
	pinned: boolean;
	href?: string;
	pdfPosition?: { page: number; scale: string };
}

class TabsStore {
	#workspaceId: string | null | undefined;
	setWorkspace(id: string | null | undefined) {
		if (this.#workspaceId !== id) {
			this.#workspaceId = id;
			this.clear();
		}
	}
	tabs = $state<Tab[]>([]);
	activeId = $state<string | null>(null);

	open(objectId: string, objectType: ObjectType, title: string) {
		const existing = this.tabs.find((t) => t.objectId === objectId);
		if (existing) {
			this.activeId = existing.id;
			return existing.id;
		}
		// Replace existing preview tab
		const previewIndex = this.tabs.findIndex((t) => !t.pinned);
		const tab: Tab = {
			id: crypto.randomUUID(),
			objectId,
			objectType,
			title,
			pinned: false,
		};
		if (previewIndex >= 0) {
			this.tabs = this.tabs.map((t, i) => (i === previewIndex ? tab : t));
		} else {
			this.tabs = [...this.tabs, tab];
		}
		this.activeId = tab.id;
		return tab.id;
	}

	openNew(objectId: string, objectType: ObjectType, title: string) {
		const tab: Tab = {
			id: crypto.randomUUID(),
			objectId,
			objectType,
			title,
			pinned: true,
		};
		this.tabs = [...this.tabs, tab];
		this.activeId = tab.id;
		return tab.id;
	}

	pin(id: string) {
		this.tabs = this.tabs.map((t) =>
			t.id === id ? { ...t, pinned: true } : t,
		);
	}

	close(id: string) {
		const index = this.tabs.findIndex((t) => t.id === id);
		const wasActive = this.activeId === id;
		this.tabs = this.tabs.filter((t) => t.id !== id);
		if (wasActive)
			this.activeId = this.tabs[Math.max(0, index - 1)]?.id ?? null;
	}

	setLocation(id: string, href: string) {
		this.tabs = this.tabs.map((tab) =>
			tab.id === id ? { ...tab, href } : tab,
		);
	}
	setPdfPosition(id: string, pdfPosition: { page: number; scale: string }) {
		this.tabs = this.tabs.map((tab) =>
			tab.id === id ? { ...tab, pdfPosition } : tab,
		);
	}
	clear() {
		this.tabs = [];
		this.activeId = null;
	}

	setActive(id: string | null) {
		this.activeId = id;
	}

	renameObject(objectId: string, title: string) {
		this.tabs = this.tabs.map((tab) =>
			tab.objectId === objectId ? { ...tab, title } : tab,
		);
	}

	get active() {
		return this.tabs.find((t) => t.id === this.activeId);
	}
}

export const tabsStore = new TabsStore();
