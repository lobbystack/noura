import { goto } from '$app/navigation';
import { page } from '$app/state';
import { tabHref, tabsStore } from './tabs.svelte';
import { saveBeforeLeaving } from './editor/unsaved-changes';

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
	// The tab on screen takes its editor with it: save what was typed first,
	// or let the user decide when it can't be saved.
	if (
		tabsStore.activeId === id &&
		!(await saveBeforeLeaving(() => removeTab(id)))
	)
		return;
	await removeTab(id);
}

async function removeTab(id: string): Promise<void> {
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
