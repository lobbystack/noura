/**
 * Workspace object-key delivery for browser devices.
 *
 * `receiveKeys` pulls the server's signed `noura.sync.key.web` envelopes for a
 * recipient device, verifies each against a caller-pinned signer key, and only
 * then unwraps it with the device recipient secret. The server is never a trust
 * source: an envelope from an unpinned signer or addressed to a different
 * recipient is rejected. `age`-construction envelopes are reported as
 * unsupported markers rather than being mis-decrypted.
 */

import {
	unwrapKey,
	verifyEnvelope,
	type WebKeyEnvelope,
} from '@noura/sync-key-envelope';
import { bytesEqual, decodeBase64, type Bytes } from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import type { FetchLike } from './http';
import { ensureResponseOk, readJson } from './http';
import type { DeviceIdentity } from './identity';

/** Safety bound on pagination rounds to avoid an unbounded loop. */
const MAX_KEY_PAGES = 1000;

/** A delivered envelope this client cannot decrypt. */
export interface UnsupportedEnvelope {
	/** Object the envelope belongs to. */
	objectId: string;
	/** Recipient device the envelope was addressed to. */
	deviceId: string;
	/** Key epoch. */
	epoch: number;
	/** Construction discriminator reported by the server. */
	construction: string;
	/** Stable marker code; always `unsupported_envelope`. */
	code: 'unsupported_envelope';
}

/** Result of {@link receiveKeys}. */
export interface ReceiveKeysResult {
	/** Unwrapped 32-byte object keys, keyed by object id. */
	keys: Map<string, Bytes>;
	/** Envelopes skipped because their construction is unsupported. */
	unsupported: UnsupportedEnvelope[];
}

/** Inputs for {@link receiveKeys}. */
export interface ReceiveKeysInput {
	/** Account origin. */
	origin: string;
	/** Device bearer token. */
	token: string;
	/** Workspace to receive keys for. */
	workspaceId: string;
	/** Recipient device identifier. */
	deviceId: string;
	/** Unlocked device identity holding the recipient secret. */
	identity: DeviceIdentity;
	/** Caller-pinned map of signing device id to raw Ed25519 public key. */
	pinnedSigners: ReadonlyMap<string, Uint8Array>;
	/** Injected transport. */
	fetch: FetchLike;
	/** Pagination cursor; defaults to the start. */
	afterObject?: string;
	/** Pagination cursor; defaults to the start. */
	afterEpoch?: number;
}

function asRecord(value: unknown): Record<string, unknown> {
	if (!value || typeof value !== 'object' || Array.isArray(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response had an unexpected shape',
		);
	}
	return value as Record<string, unknown>;
}

function requireString(value: unknown, field: string): string {
	if (typeof value !== 'string' || value.length === 0) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			`response was missing ${field}`,
		);
	}
	return value;
}

function requireEpoch(value: unknown): number {
	const epoch =
		typeof value === 'string' && /^[0-9]+$/.test(value) ? Number(value) : value;
	if (!Number.isSafeInteger(epoch) || (epoch as number) < 1) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response carried an invalid epoch',
		);
	}
	return epoch as number;
}

function rowEnvelope(
	row: Record<string, unknown>,
	workspaceId: string,
): WebKeyEnvelope {
	return {
		workspace_id: workspaceId,
		object_id: requireString(row.objectId, 'objectId'),
		epoch: requireEpoch(row.epoch),
		signing_device: requireString(row.signingDevice, 'signingDevice'),
		device_id: requireString(row.deviceId, 'deviceId'),
		recipient_public_key: requireString(
			row.recipientPublicKey,
			'recipientPublicKey',
		),
		ephemeral_public_key: requireString(
			row.ephemeralPublicKey,
			'ephemeralPublicKey',
		),
		salt: requireString(row.salt, 'salt'),
		nonce: requireString(row.nonce, 'nonce'),
		wrapped_key: requireString(row.wrappedKey, 'wrappedKey'),
		signature: requireString(row.signature, 'signature'),
	};
}

async function unwrapPinned(
	envelope: WebKeyEnvelope,
	identity: DeviceIdentity,
	pinnedSigners: ReadonlyMap<string, Uint8Array>,
): Promise<Bytes> {
	if (envelope.device_id !== identity.deviceId) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.RecipientMismatch,
			'envelope was addressed to a different device',
		);
	}
	let recipientPublic: Bytes;
	try {
		recipientPublic = decodeBase64(envelope.recipient_public_key, 32);
	} catch {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'envelope recipient key was malformed',
		);
	}
	if (!bytesEqual(recipientPublic, identity.x25519Public)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.RecipientMismatch,
			'envelope recipient key did not match this device',
		);
	}

	const signer = pinnedSigners.get(envelope.signing_device);
	if (!signer) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UnpinnedSigner,
			'envelope signer was not pinned',
		);
	}
	const signerBytes = new Uint8Array(signer);
	if (signerBytes.length !== 32) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'pinned signer key had the wrong length',
		);
	}

	try {
		await verifyEnvelope(envelope, signerBytes);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'envelope signature did not verify',
			{ cause },
		);
	}

	let objectKey: Bytes;
	try {
		objectKey = await unwrapKey(envelope, identity.x25519Secret, signerBytes);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'envelope could not be unwrapped',
			{ cause },
		);
	}
	if (objectKey.length !== 32) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'unwrapped object key had the wrong length',
		);
	}
	return objectKey;
}

/**
 * Fetch, verify against pinned signers, and unwrap workspace object keys.
 *
 * Paginates `GET /v1/workspaces/:workspace/keys` with `afterObject` and
 * `afterEpoch` until the server reports no more envelopes. Every web envelope is
 * verified before decryption. An envelope from an unpinned signer or addressed
 * to another recipient rejects the call; an `age` or unknown-construction
 * envelope is returned as an {@link UnsupportedEnvelope} marker.
 */
export async function receiveKeys(
	input: ReceiveKeysInput,
): Promise<ReceiveKeysResult> {
	const keys = new Map<string, Bytes>();
	const unsupported: UnsupportedEnvelope[] = [];
	let afterObject = input.afterObject ?? '';
	let afterEpoch = input.afterEpoch ?? 0;
	let hasMore = true;

	for (let page = 0; page < MAX_KEY_PAGES; page += 1) {
		const url = new URL(
			`/v1/workspaces/${encodeURIComponent(input.workspaceId)}/keys`,
			input.origin,
		);
		url.searchParams.set('device', input.deviceId);
		// The server treats a present-but-empty `afterObject` as an identifier and
		// rejects it, so omit the parameter entirely on the first page, matching the
		// native client.
		if (afterObject !== '') url.searchParams.set('afterObject', afterObject);
		url.searchParams.set('afterEpoch', String(afterEpoch));

		const response = await input.fetch(url.toString(), {
			method: 'GET',
			credentials: 'include',
			headers: {
				authorization: `Bearer ${input.token}`,
				accept: 'application/json',
			},
		});
		ensureResponseOk(response);
		const data = asRecord(await readJson(response));
		if (!Array.isArray(data.envelopes)) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidResponse,
				'response was missing envelopes',
			);
		}
		// Mirror the native client: a page is at most 100 envelopes, and a
		// `hasMore` page that carries none cannot advance the cursor.
		if (
			data.envelopes.length > 100 ||
			(data.hasMore === true && data.envelopes.length === 0)
		) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidResponse,
				'key page was malformed',
			);
		}

		for (const raw of data.envelopes) {
			const row = asRecord(raw);
			const objectId = requireString(row.objectId, 'objectId');
			const epoch = requireEpoch(row.epoch);
			// Envelopes are ordered by (object id, epoch) and every row must move
			// the cursor strictly forward. A repeated or out-of-order cursor would
			// otherwise loop on a page and return a partial key set.
			if (
				objectId < afterObject ||
				(objectId === afterObject && epoch <= afterEpoch)
			) {
				throw new BrowserSyncError(
					BrowserSyncErrorCode.InvalidResponse,
					'key envelopes were not in ascending order',
				);
			}
			afterObject = objectId;
			afterEpoch = epoch;

			const construction =
				typeof row.construction === 'string' ? row.construction : 'age';
			if (construction !== 'web') {
				unsupported.push({
					objectId,
					deviceId: requireString(row.deviceId, 'deviceId'),
					epoch,
					construction,
					code: BrowserSyncErrorCode.UnsupportedEnvelope,
				});
				continue;
			}
			const envelope = rowEnvelope(row, input.workspaceId);
			keys.set(
				envelope.object_id,
				await unwrapPinned(envelope, input.identity, input.pinnedSigners),
			);
		}

		hasMore = data.hasMore === true;
		if (!hasMore) break;
	}

	if (hasMore) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'key pagination exceeded the maximum page count',
		);
	}

	return { keys, unsupported };
}
