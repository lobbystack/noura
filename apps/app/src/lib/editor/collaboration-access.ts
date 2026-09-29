import type { CollaborationProvider } from '@noura/plugin-sdk';
import {
	CollaborationRegistry,
	type CollaborationLease,
	type NativeCollaborationClient,
} from './collaboration-registry';

type RegisterDraft = ConstructorParameters<typeof CollaborationRegistry>[1];

export interface CollaborationAccessOptions {
	/** Where the active sync plugin registers its provider. */
	slot: { readonly provider: CollaborationProvider | null };
	events: NativeCollaborationClient['events'];
	registerDraft: RegisterDraft;
	/** Identifies the open workspace, or null when none is open. */
	scope: () => string | null;
}

export interface CollaborationAccess {
	/** Whether a collaboration provider is registered right now. */
	readonly available: boolean;
	/**
	 * Lease the collaboration session for `relativePath`. Resolves to null
	 * without any native request when no provider is registered, and to null
	 * when the file is not a collaborative document.
	 */
	acquire(relativePath: string): Promise<CollaborationLease | null>;
}

/**
 * The editors' only way into collaboration. Sessions are shared per provider
 * and workspace, so a new provider (the sync plugin turning off and on
 * again) never reuses sessions opened through the previous one.
 */
export function createCollaborationAccess(
	options: CollaborationAccessOptions,
): CollaborationAccess {
	const registries = new WeakMap<
		CollaborationProvider,
		Map<string, CollaborationRegistry>
	>();
	return {
		get available() {
			return options.slot.provider !== null;
		},
		async acquire(relativePath) {
			const provider = options.slot.provider;
			if (!provider) return null;
			const scope = options.scope();
			if (!scope) throw new Error('No workspace is open');
			let scoped = registries.get(provider);
			if (!scoped) {
				scoped = new Map();
				registries.set(provider, scoped);
			}
			let registry = scoped.get(scope);
			if (!registry) {
				registry = new CollaborationRegistry(
					{ collaboration: provider, events: options.events },
					options.registerDraft,
				);
				scoped.set(scope, registry);
			}
			return registry.acquire(relativePath);
		},
	};
}
