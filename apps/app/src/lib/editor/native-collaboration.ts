import { getNouraClient, workspace } from '$lib/state.svelte';
import { registerPendingDraft } from './pending-drafts.svelte';
import { CollaborationRegistry } from './collaboration-registry';

const registries = new WeakMap<object, Map<string, CollaborationRegistry>>();
export function acquireNativeCollaboration(relativePath: string) {
	const client = getNouraClient();
	const scope = workspace.state;
	if (!scope?.workspaceId || !scope.rootPath)
		throw new Error('No workspace is open');
	const key = JSON.stringify([scope.workspaceId, scope.rootPath]);
	let scoped = registries.get(client);
	if (!scoped) {
		scoped = new Map();
		registries.set(client, scoped);
	}
	let registry = scoped.get(key);
	if (!registry) {
		registry = new CollaborationRegistry(client, registerPendingDraft);
		scoped.set(key, registry);
	}
	return registry.acquire(relativePath);
}
