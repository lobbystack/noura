/**
 * Public result, status, and binding shapes for the browser sync controller.
 */

import type { DeviceIdentity, FetchLike } from '@noura/browser-sync';
import type {
	BrowserSyncRemote,
	BrowserSyncStorage,
	SyncStateStore,
} from '@noura/browser-sync-engine';
import { runBrowserSyncReconcile } from './reconcile';

/** Controller custody and availability state. */
export type BrowserSyncStatus =
	'unavailable' | 'locked' | 'unlocked' | 'enrolled' | 'error';

/** Public, non-secret device projection. */
export interface BrowserSyncDeviceInfo {
	deviceId: string;
	enrolled: boolean;
}

/** Failure discriminator for controller actions. */
export type BrowserSyncFailureCode =
	| 'unavailable'
	| 'not_configured'
	| 'locked'
	| 'invalid_passphrase'
	| 'passphrase_rejected'
	| 'enroll_failed'
	| 'custody_failed'
	| 'device_not_found'
	| 'fingerprint_mismatch'
	| 'sync_failed'
	| 'attachment_failed'
	| 'attachment_too_large'
	| 'attachment_unavailable'
	| 'object_not_found';

/** Uniform typed result for custody actions. */
export type BrowserSyncResult<T = undefined> =
	| { ok: true; value: T }
	| { ok: false; code: BrowserSyncFailureCode; message: string };

/** Outcome of {@link BrowserSyncController.syncNow}. */
export type SyncNowOutcome =
	| {
			status: 'synced';
			pushed: number;
			applied: number;
			conflicts: number;
			cursor: string;
			/** Local files with no owning object; skipped, never sealed under another object. */
			skippedUnmanaged: number;
	  }
	| { status: 'unavailable'; message: string }
	| { status: 'locked'; message: string }
	| { status: 'not_configured'; message: string }
	| { status: 'revoked'; message: string }
	| { status: 'error'; message: string };

/** One recorded conflict, projected for the workspace summary. */
export interface BrowserSyncConflictDetail {
	operationId: string;
	objectId: string;
	path: string;
	reason: string;
}

/** Read-only counters for the currently bound workspace replica. */
export interface BrowserSyncWorkspaceSummary {
	configured: boolean;
	workspaceId: string | null;
	cursor: string;
	pending: number;
	conflicts: number;
	/** Recorded conflicts with enough detail to choose a resolution. */
	conflictDetails: BrowserSyncConflictDetail[];
}

/** Public, non-secret projection of one device in a workspace access state. */
export interface BrowserSyncDeviceCard {
	deviceId: string;
	accountId: string;
	publicKey: string;
	encryptionRecipient: string;
	fingerprint: string;
	approved: boolean;
}

/** One remote sync object bound to this browser workspace. */
export interface BrowserSyncObjectBinding {
	/** Stable remote object ID this entry wraps a key for. */
	objectId: string;
	/** Canonical path the object owned when it was bound. */
	path: string;
	/** Stable local object ID, used to follow a move to a new path. */
	localObjectId?: string;
	/** Current local path from the managed-object list, when it differs from `path`. */
	livePath?: string;
	/**
	 * True when this object was recovered from a native kit and has no verified
	 * canonical local path. Unmapped objects can open delivered operations but
	 * never own a local file or attachment.
	 */
	unmapped?: boolean;
	/** Positive safe-integer key epoch. */
	epoch: number;
	/** Canonical decimal access-policy revision the object was bound at. */
	policyRevision: string;
}

/**
 * The injectable boundaries one browser workspace replica needs. A host binds
 * these once `BrowserWorkspaceStorage` is reachable from the same thread as the
 * controller (today it is not, so the hosted UI leaves this unset).
 */
export interface BrowserSyncWorkspaceBinding {
	/** Stable workspace id. */
	workspaceId: string;
	/**
	 * Primary/default object used when {@link objects} is absent. Kept so a
	 * single-object binding (including one persisted before per-object sync)
	 * keeps working unchanged.
	 */
	objectId: string;
	/** Positive safe-integer key epoch of the primary object. */
	epoch: number;
	/** Canonical decimal access-policy revision of the primary object. */
	policyRevision: string;
	/** Object keys by object id; a key must exist for `objectId`. */
	objectKeys: ReadonlyMap<string, Uint8Array>;
	/**
	 * Per-object bindings by object id. When present and non-empty, a file change
	 * is sealed under the object whose current or last-known path matches. When
	 * absent, every change is sealed under the primary `objectId`.
	 */
	objects?: ReadonlyMap<string, BrowserSyncObjectBinding>;
	/** Pinned Ed25519 public keys by signing device id. */
	pinnedSigners: ReadonlyMap<string, Uint8Array>;
	/** Local replica boundary. */
	storage: BrowserSyncStorage;
	/** Durable state boundary. */
	state: SyncStateStore;
	/** Encrypted remote boundary. */
	remote: BrowserSyncRemote;
	/** Clock used to timestamp conflicts. */
	now?: () => number;
	/**
	 * Transport metadata used to download and decrypt version-3 attachment
	 * ciphertext during reconcile. Absent when the host did not supply an origin
	 * and token; a version-3 change is then refused rather than written empty.
	 */
	attachments?: {
		origin: string;
		token: string;
		fetch: FetchLike;
	};
}

/** Inputs for {@link runBrowserSyncReconcile}. */
export interface BrowserSyncReconcileInput extends BrowserSyncWorkspaceBinding {
	/** Unlocked device identity holding the signing seed. */
	identity: DeviceIdentity;
	/** Called once if revocation locks the engine, so the host can clear keys. */
	onRevoked?: (error: unknown) => void | Promise<void>;
}
