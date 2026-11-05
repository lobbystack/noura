import { toast } from 'svelte-sonner';
import { isCoreError, type Task } from '@noura/workspace';
import { flushPendingDrafts } from './editor/pending-drafts.svelte';
import { getNouraClient } from './state.svelte';
import { tabsStore } from './tabs.svelte';

/**
 * Task commands shared by the task list, the project board and their menus.
 * Each reports problems as a toast in plain words.
 */

/** Create a task named `title`. Resolves to null when it failed. */
export async function createTask(
	title: string,
	properties?: Record<string, unknown>,
): Promise<Task | null> {
	try {
		const result = await getNouraClient().tasks.create({
			title,
			...(properties ? { properties } : {}),
		});
		return result.value as Task;
	} catch {
		toast.error(`Couldn’t create “${title}”.`);
		return null;
	}
}

/** Rename a task. Resolves to false when it failed. */
export async function renameTask(task: Task, title: string): Promise<boolean> {
	if (!(await flushPendingDrafts())) {
		toast.error('Save or discard your changes first, then try again.');
		return false;
	}
	const client = getNouraClient();
	try {
		// Rename the latest version: an open task may have saved since the
		// list last loaded.
		const latest = await client.tasks.get(task.id);
		await client.tasks.update(task.id, {
			title,
			expectedRevision: latest.revision,
		});
	} catch {
		toast.error(`Couldn’t rename “${task.title}”.`);
		return false;
	}
	tabsStore.renameObject(task.id, title);
	return true;
}

/** Move a task's file to the system trash and close its tabs. */
export async function trashTask(task: Task): Promise<boolean> {
	// Save what is typed so the trashed copy has it; a failed save doesn't
	// stop a deliberate delete.
	await flushPendingDrafts();
	let fallback: string | null;
	try {
		fallback = await getNouraClient().files.trash({
			relativePath: task.relativePath,
		});
	} catch (error) {
		toast.error(
			isCoreError(error) && error.code === 'collaboration_transaction_required'
				? 'noura can’t delete a task while it’s open for live editing.'
				: `Couldn’t move “${task.title}” to the trash.`,
		);
		return false;
	}
	for (const tab of tabsStore.tabs.filter((tab) => tab.objectId === task.id))
		tabsStore.close(tab.id);
	toast.success(
		fallback
			? `Moved “${task.title}” to .noura/trash in this workspace.`
			: `Moved “${task.title}” to the trash.`,
	);
	return true;
}
