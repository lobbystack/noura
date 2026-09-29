import { isCoreError } from '@noura/workspace';
import {
	assertFreshIdentity,
	decodeBackup,
	downloadBackup,
	encodeBackup,
	IDENTITY_CONFLICT,
} from './browser-backup';
import { getBrowserWorkspace } from './browser-workspace';
import { flushPendingDrafts } from './editor/pending-drafts.svelte';
import { workspace } from './state.svelte';

const SAVE_FIRST = 'Save your changes first, then try again.';

/** Download the open browser workspace as a backup file. */
export async function downloadWorkspaceBackup(): Promise<void> {
	if (!(await flushPendingDrafts())) throw new Error(SAVE_FIRST);
	const snapshot = await getBrowserWorkspace().exportWorkspace();
	downloadBackup(encodeBackup(snapshot), workspace.name);
}

/**
 * Restore a backup file as a new workspace in this browser and open it. A
 * workspace that is already here is never overwritten.
 */
export async function restoreWorkspaceBackup(file: Blob): Promise<void> {
	if (!(await flushPendingDrafts())) throw new Error(SAVE_FIRST);
	const snapshot = await decodeBackup(file);
	await workspace.refreshRecents();
	assertFreshIdentity(snapshot.workspaceId, workspace.recents);
	try {
		await workspace.importBrowserBackup(snapshot);
	} catch (error) {
		if (isCoreError(error) && error.code === 'workspace_not_empty')
			throw new Error(IDENTITY_CONFLICT);
		throw error;
	}
}

/** Whether the browser promised to keep this site's files. */
export async function storageIsPersistent(): Promise<boolean> {
	try {
		return (await navigator.storage?.persisted?.()) ?? false;
	} catch {
		return false;
	}
}

/** Ask the browser to keep this site's files when space runs low. */
export async function requestPersistentStorage(): Promise<boolean> {
	try {
		return (await navigator.storage?.persist?.()) ?? false;
	} catch {
		return false;
	}
}
