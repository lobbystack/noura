/**
 * Durable browser sync binding record.
 *
 * The record is durable adapter state that lives in origin storage outside
 * canonical workspace files. It holds no unwrapped key material: an object key
 * is always persisted as a self-wrapped `noura.sync.key.web` envelope addressed
 * to the browser device, and pinned signers are public Ed25519 keys.
 *
 * The record is shared with the browser recovery kit, which encrypts it for
 * offline restore, and its validator is shared by the storage read path and
 * recovery-kit import so the two cannot drift.
 */

/**
 * Wrapped object-key material persisted in the binding record.
 *
 * This is the `noura.sync.key.web` envelope bound to the browser's own device,
 * so it can be unwrapped only after the device bundle is unlocked.
 */
export interface BrowserSyncBoundKey {
	deviceId: string;
	wrappedKey: string;
	signature: string;
	construction: 'web';
	recipientPublicKey: string;
	ephemeralPublicKey: string;
	salt: string;
	nonce: string;
}

/** One remote sync object bound to a browser workspace. */
export interface BrowserSyncBoundObject {
	/** Canonical-path anchor for the object at bind time. */
	path: string;
	/** Stable local object ID, used to follow a move to a new path. */
	localObjectId?: string;
	/** Positive safe-integer object key epoch. */
	epoch: number;
	/** Canonical access-policy revision the object was bound at. */
	policyRevision: string;
	/**
	 * True when this object came from a recovery import and has no verified
	 * canonical local path. An unmapped object may decrypt delivered operations
	 * but must never be treated as the owner of a local file or attachment, so a
	 * crafted recovery kit cannot cause local bytes to be sealed under an
	 * attacker-known key.
	 */
	unmapped?: boolean;
	/** Self-wrapped object key; never plaintext. */
	key: BrowserSyncBoundKey;
}

/** Durable browser sync binding record. Contains no unwrapped key material. */
export interface BrowserSyncBindingRecord {
	version: 1;
	/** Stable ID of the local browser workspace this binding belongs to. */
	localWorkspaceId: string;
	/** Remote workspace identifier. */
	workspaceId: string;
	/** Access-policy revision last persisted. */
	revision: string;
	/** Primary sync object carrying this workspace's file changes. */
	objectId: string;
	/** Object bindings by object id. */
	objects: Record<string, BrowserSyncBoundObject>;
	/** Pinned signer public keys by device id, base64. */
	pinnedSigners: Record<string, string>;
}

/**
 * True when `value` matches the binding record shape.
 *
 * Every bound object and its wrapped key are validated, the primary `objectId`
 * must exist in `objects`, and every pinned signer must be a string. This does
 * not verify signatures or decrypt anything, so it is a structural check only.
 */
export function isBrowserSyncBindingRecord(
	value: unknown,
): value is BrowserSyncBindingRecord {
	if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
	const record = value as Record<string, unknown>;
	if (
		record.version !== 1 ||
		typeof record.localWorkspaceId !== 'string' ||
		typeof record.workspaceId !== 'string' ||
		typeof record.revision !== 'string' ||
		typeof record.objectId !== 'string' ||
		!record.objects ||
		typeof record.objects !== 'object' ||
		Array.isArray(record.objects) ||
		!record.pinnedSigners ||
		typeof record.pinnedSigners !== 'object' ||
		Array.isArray(record.pinnedSigners)
	)
		return false;
	const objects = record.objects as Record<string, unknown>;
	for (const raw of Object.values(objects)) {
		if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return false;
		const bound = raw as Record<string, unknown>;
		if (
			typeof bound.path !== 'string' ||
			(bound.localObjectId !== undefined &&
				typeof bound.localObjectId !== 'string') ||
			(bound.unmapped !== undefined && typeof bound.unmapped !== 'boolean') ||
			!Number.isSafeInteger(bound.epoch) ||
			(bound.epoch as number) < 1 ||
			typeof bound.policyRevision !== 'string' ||
			!bound.key ||
			typeof bound.key !== 'object' ||
			Array.isArray(bound.key)
		)
			return false;
		const key = bound.key as Record<string, unknown>;
		if (
			typeof key.deviceId !== 'string' ||
			typeof key.wrappedKey !== 'string' ||
			typeof key.signature !== 'string' ||
			key.construction !== 'web' ||
			typeof key.recipientPublicKey !== 'string' ||
			typeof key.ephemeralPublicKey !== 'string' ||
			typeof key.salt !== 'string' ||
			typeof key.nonce !== 'string'
		)
			return false;
	}
	if (
		!(record.objectId in objects) ||
		!Object.values(record.pinnedSigners).every(
			(entry) => typeof entry === 'string',
		)
	)
		return false;
	return true;
}
