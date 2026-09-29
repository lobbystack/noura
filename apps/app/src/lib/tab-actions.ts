import { goto } from '$app/navigation';
import { page } from '$app/state';
import { tabHref, tabsStore } from './tabs.svelte';

/** Show a tab's document. */
export async function activateTab(id: string): Promise<void> {
	const tab = tabsStore.tabs.find((candidate) => candidate.id === id);
	if (!tab) return;
	tabsStore.setActive(id);
	await goto(tabHref(tab));
}

/**
 * Close a tab. Closing the one on screen shows its neighbor, or the same
 * page with nothing open when it was the last tab.
 */
export async function closeTab(id: string): Promise<void> {
	const wasActive = tabsStore.activeId === id;
	tabsStore.close(id);
	if (!wasActive) return;
	const next = tabsStore.active;
	if (next) await goto(tabHref(next));
	else await goto(page.url.pathname);
}

export async function closeActiveTab(): Promise<void> {
	if (tabsStore.activeId) await closeTab(tabsStore.activeId);
}

/** Move to the next (1) or previous (-1) tab, wrapping around. */
export async function cycleTab(offset: 1 | -1): Promise<void> {
	const next = tabsStore.neighbor(offset);
	if (next && next.id !== tabsStore.activeId) await activateTab(next.id);
}
