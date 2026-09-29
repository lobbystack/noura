/**
 * Conversions between `noura.sync.key.web` envelopes, the persisted binding
 * shape, and the signed access-policy shape.
 */

import { unwrapKey, type WebKeyEnvelope } from '@noura/sync-key-envelope';
import type { AccessPolicyEnvelope } from './access-policy';
import type {
	BrowserSyncBindingRecord,
	BrowserSyncBoundKey,
	BrowserSyncBoundObject,
} from './binding';
import type { DeviceIdentity } from './identity';

/** Convert a self-wrapped key envelope into the persisted binding shape. */
export function toBoundKey(
	envelope: WebKeyEnvelope,
	deviceId: string,
): BrowserSyncBoundKey {
	return {
		deviceId,
		wrappedKey: envelope.wrapped_key,
		signature: envelope.signature,
		construction: 'web',
		recipientPublicKey: envelope.recipient_public_key,
		ephemeralPublicKey: envelope.ephemeral_public_key,
		salt: envelope.salt,
		nonce: envelope.nonce,
	};
}

/** Convert a wrapped key envelope into the signed access-policy shape. */
export function toPolicyEnvelope(
	envelope: WebKeyEnvelope,
	deviceId: string,
): AccessPolicyEnvelope {
	return {
		deviceId,
		wrappedKey: envelope.wrapped_key,
		signature: envelope.signature,
		construction: 'web',
		recipientPublicKey: envelope.recipient_public_key,
		ephemeralPublicKey: envelope.ephemeral_public_key,
		salt: envelope.salt,
		nonce: envelope.nonce,
	};
}

/** Convert a persisted binding entry back into a verifiable key envelope. */
export function toWebEnvelope(
	record: BrowserSyncBindingRecord,
	objectId: string,
	bound: BrowserSyncBoundObject,
): WebKeyEnvelope {
	return {
		workspace_id: record.workspaceId,
		object_id: objectId,
		epoch: bound.epoch,
		signing_device: bound.key.deviceId,
		device_id: bound.key.deviceId,
		recipient_public_key: bound.key.recipientPublicKey,
		ephemeral_public_key: bound.key.ephemeralPublicKey,
		salt: bound.key.salt,
		nonce: bound.key.nonce,
		wrapped_key: bound.key.wrappedKey,
		signature: bound.key.signature,
	};
}

/** Verify and unwrap one persisted object key with the unlocked identity. */
export async function unwrapBoundKey(
	record: BrowserSyncBindingRecord,
	objectId: string,
	bound: BrowserSyncBoundObject,
	identity: DeviceIdentity,
): Promise<Uint8Array> {
	return unwrapKey(
		toWebEnvelope(record, objectId, bound),
		identity.x25519Secret,
		identity.signingPublic,
	);
}
