/** Composes the concrete engine boundaries for one browser workspace. */

import { createSameOriginFetch, type FetchLike } from '@noura/browser-sync';
import {
	createWorkerWorkspaceStorage,
	createWorkspaceSyncStorage,
	type SyncStateStore,
	type WorkspaceStorageLike,
} from '@noura/browser-sync-engine';
import { openBrowserSyncStateStore } from './opfs';
import { createBrowserSyncRemote } from './remote';
import type {
	BrowserSyncObjectBinding,
	BrowserSyncWorkspaceBinding,
} from './types';

/** Inputs for {@link createBrowserSyncWorkspaceBinding}. */
export interface BrowserSyncWorkspaceSource {
	/** Stable workspace id. */
	workspaceId: string;
	/** Object that carries this replica's file changes. */
	objectId: string;
	/** Positive safe-integer key epoch. */
	epoch: number;
	/** Canonical decimal access-policy revision. */
	policyRevision: string;
	/** Object keys by object id; a key must exist for `objectId`. */
	objectKeys: ReadonlyMap<string, Uint8Array>;
	/** Per-object bindings by object id. */
	objects?: ReadonlyMap<string, BrowserSyncObjectBinding>;
	/** Pinned Ed25519 public keys by signing device id. */
	pinnedSigners: ReadonlyMap<string, Uint8Array>;
	/** The workspace replica. In the hosted app it lives in the workspace worker. */
	workspace: WorkspaceStorageLike;
	/** Account origin. */
	origin: string;
	/** Device bearer token. */
	token: string;
	/** Injected transport. Defaults to a same-origin `fetch` wrapper. */
	fetch?: FetchLike;
	/** Durable state store. Defaults to {@link openBrowserSyncStateStore}. */
	state?: SyncStateStore;
	/** Clock used to timestamp conflicts. */
	now?: () => number;
}

/**
 * Compose the concrete engine boundaries for one browser workspace.
 *
 * This uses {@link createWorkspaceSyncStorage} over the workspace replica, an
 * engine file-system state store, and a `BrowserSyncTransport`-backed remote.
 * It returns `null` when no durable state store can be opened (no OPFS), so the
 * caller can report an unsupported state instead of syncing against volatile
 * state.
 *
 * The hosted app supplies `source.workspace` through
 * {@link createWorkerWorkspaceStorage} over the workspace worker's raw file
 * operations.
 */
export async function createBrowserSyncWorkspaceBinding(
	source: BrowserSyncWorkspaceSource,
): Promise<BrowserSyncWorkspaceBinding | null> {
	const state =
		source.state ?? (await openBrowserSyncStateStore(source.workspaceId));
	if (!state) return null;
	return {
		workspaceId: source.workspaceId,
		objectId: source.objectId,
		epoch: source.epoch,
		policyRevision: source.policyRevision,
		objectKeys: source.objectKeys,
		...(source.objects === undefined ? {} : { objects: source.objects }),
		pinnedSigners: source.pinnedSigners,
		storage: createWorkspaceSyncStorage(source.workspace),
		state,
		remote: createBrowserSyncRemote({
			origin: source.origin,
			token: source.token,
			workspaceId: source.workspaceId,
			...(source.fetch === undefined ? {} : { fetch: source.fetch }),
		}),
		attachments: {
			origin: source.origin,
			token: source.token,
			fetch: source.fetch ?? createSameOriginFetch(source.origin),
		},
		...(source.now === undefined ? {} : { now: source.now }),
	};
}
