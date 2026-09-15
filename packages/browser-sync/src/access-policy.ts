/**
 * Browser-side signed access-policy construction and verification.
 *
 * The canonical signing bytes match the server's `accessSigningBytes` and the
 * native policy builder byte-for-byte. Signing uses the device Ed25519 key;
 * browser (`web`) envelopes are verified against the same `noura.sync.key.web`
 * tuple as key delivery.
 */

import {
	KeyEnvelopeError,
	verifyEnvelope,
	type WebKeyEnvelope,
} from '@noura/sync-key-envelope';
import { importSigningKey, importVerifyKey } from './crypto';
import type { Bytes } from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import type { DeviceIdentity } from './identity';

/** One recipient envelope inside an access policy object entry. */
export interface AccessPolicyEnvelope {
	deviceId: string;
	wrappedKey: string;
	signature: string;
	construction?: 'age' | 'web';
	recipientPublicKey?: string;
	ephemeralPublicKey?: string;
	salt?: string;
	nonce?: string;
}

/** One object entry inside an access policy. */
export interface AccessPolicyObject {
	objectId: string;
	epoch: number;
	grants: Array<{ accountId: string; role: 'editor' | 'viewer' }>;
	envelopes: AccessPolicyEnvelope[];
	document?: { generation: string; mode: 'text' | 'attachment' };
}

/** A signed access policy as accepted by `PUT /v1/workspaces/:workspace/access`. */
export interface AccessPolicy {
	version: 1 | 2;
	workspaceId: string;
	revision: string;
	previousPolicyDigest: string | null;
	deviceId: string;
	members: Array<{
		accountId: string;
		role: 'owner' | 'admin' | 'editor' | 'viewer';
	}>;
	objects: AccessPolicyObject[];
	signature: string;
}

type SigningValue =
	string | number | null | SigningValue[] | { [key: string]: SigningValue };

function envelopeTuple(envelope: AccessPolicyEnvelope): SigningValue[] {
	if (envelope.construction === 'web') {
		return [
			envelope.deviceId,
			envelope.wrappedKey,
			envelope.signature,
			'web',
			envelope.recipientPublicKey ?? '',
			envelope.ephemeralPublicKey ?? '',
			envelope.salt ?? '',
			envelope.nonce ?? '',
		];
	}
	return [envelope.deviceId, envelope.wrappedKey, envelope.signature];
}

/**
 * Canonical UTF-8 signing bytes for an access policy without its signature.
 *
 * Field order and structure mirror the server exactly; do not reorder fields.
 */
export function accessSigningBytes(
	policy: Omit<AccessPolicy, 'signature'>,
): Bytes {
	const tuple: SigningValue[] = [
		'noura.sync.access',
		policy.version,
		policy.workspaceId,
		policy.revision,
		policy.previousPolicyDigest,
		policy.deviceId,
		policy.members.map((member) => [member.accountId, member.role]),
		policy.objects.map((object) => {
			const entry: SigningValue[] = [
				object.objectId,
				object.epoch,
				object.grants.map((grant) => [grant.accountId, grant.role]),
				object.envelopes.map(envelopeTuple),
			];
			if (policy.version === 2 && object.document) {
				entry.push([object.document.generation, object.document.mode]);
			}
			return entry;
		}),
	];
	return new TextEncoder().encode(JSON.stringify(tuple));
}

/** Sign an access policy with the device signing key. */
export async function signAccessPolicy(
	policy: Omit<AccessPolicy, 'signature'>,
	identity: DeviceIdentity,
): Promise<AccessPolicy> {
	const key = await importSigningKey(identity.signingSeed);
	const signature = await globalThis.crypto.subtle.sign(
		{ name: 'Ed25519' },
		key,
		accessSigningBytes(policy),
	);
	return {
		...policy,
		signature: base64(new Uint8Array(signature)),
	};
}

/** Verify a policy signature and every recipient envelope signature. */
export async function verifyAccessPolicy(
	policy: AccessPolicy,
	signingPublic: Bytes,
): Promise<void> {
	const key = await importVerifyKey(signingPublic);
	const valid = await globalThis.crypto.subtle.verify(
		{ name: 'Ed25519' },
		key,
		decodeBase64(policy.signature),
		accessSigningBytes(policy),
	);
	if (!valid)
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidSignature,
			'access policy signature did not verify',
		);
	for (const object of policy.objects) {
		for (const envelope of object.envelopes) {
			if (envelope.construction !== 'web') continue;
			try {
				await verifyEnvelope(
					envelopeToWeb(envelope, policy, object),
					signingPublic,
				);
			} catch (error) {
				if (error instanceof KeyEnvelopeError) {
					throw new BrowserSyncError(
						BrowserSyncErrorCode.InvalidSignature,
						'access policy envelope signature did not verify',
					);
				}
				throw error;
			}
		}
	}
}

function envelopeToWeb(
	envelope: AccessPolicyEnvelope,
	policy: AccessPolicy,
	object: AccessPolicyObject,
): WebKeyEnvelope {
	return {
		workspace_id: policy.workspaceId,
		object_id: object.objectId,
		epoch: object.epoch,
		signing_device: policy.deviceId,
		device_id: envelope.deviceId,
		recipient_public_key: envelope.recipientPublicKey ?? '',
		ephemeral_public_key: envelope.ephemeralPublicKey ?? '',
		salt: envelope.salt ?? '',
		nonce: envelope.nonce ?? '',
		wrapped_key: envelope.wrappedKey,
		signature: envelope.signature,
	};
}

function base64(bytes: Uint8Array): string {
	let binary = '';
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return btoa(binary);
}

function decodeBase64(value: string): Bytes {
	let binary: string;
	try {
		binary = atob(value);
	} catch {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidSignature,
			'signature was not valid base64',
		);
	}
	const bytes = new Uint8Array(binary.length);
	for (let index = 0; index < binary.length; index += 1)
		bytes[index] = binary.charCodeAt(index);
	return bytes;
}
