import { getNouraClient, getPluginRuntime, workspace } from '$lib/state.svelte';
import { registerPendingDraft } from './pending-drafts.svelte';
import {
	createCollaborationAccess,
	type CollaborationAccess,
} from './collaboration-access';

let access: CollaborationAccess | null = null;

/** Collaboration for the open native workspace, gated by the sync plugin. */
export function getCollaborationAccess(): CollaborationAccess {
	if (!access)
		access = createCollaborationAccess({
			slot: getPluginRuntime().collaboration,
			events: getNouraClient().events,
			registerDraft: registerPendingDraft,
			scope: () => {
				const state = workspace.state;
				return state?.workspaceId && state.rootPath
					? JSON.stringify([state.workspaceId, state.rootPath])
					: null;
			},
		});
	return access;
}
