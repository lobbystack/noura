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
import type { AccessState } from './operations';

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
			if (policy.version === 2) {
				// The server signs a document tuple for every version-2 object
				// and rejects one without a document descriptor. Reject here too
				// so the two implementations cannot silently diverge.
				if (!object.document) {
					throw new BrowserSyncError(
						BrowserSyncErrorCode.InvalidPolicy,
						'version-2 access policy object was missing its document descriptor',
					);
				}
				entry.push([object.document.generation, object.document.mode]);
			}
			return entry;
		}),
	];
	return new TextEncoder().encode(JSON.stringify(tuple));
}

/**
 * SHA-256 digest of a signed access policy, as lowercase hex.
 *
 * This mirrors the server's `accessDigest` byte-for-byte: the canonical signing
 * bytes concatenated with the raw decoded signature. A policy chains to its
 * predecessor through `previousPolicyDigest`, so the digest must cover both the
 * signed content and the signature.
 */
export async function accessDigest(policy: AccessPolicy): Promise<string> {
	const signingBytes = accessSigningBytes(policy);
	const signature = decodeBase64(policy.signature);
	const combined = new Uint8Array(signingBytes.length + signature.length);
	combined.set(signingBytes, 0);
	combined.set(signature, signingBytes.length);
	const digest = await globalThis.crypto.subtle.digest('SHA-256', combined);
	let hex = '';
	for (const byte of new Uint8Array(digest)) {
		hex += byte.toString(16).padStart(2, '0');
	}
	return hex;
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

/** True when access-state lists an active device with a non-browser recipient. */
export function hasNativeActiveDevice(state: AccessState): boolean {
	for (const raw of state.devices) {
		if (!raw || typeof raw !== 'object' || Array.isArray(raw)) continue;
		const recipient = (raw as Record<string, unknown>).encryptionRecipient;
		if (
			typeof recipient === 'string' &&
			recipient.length > 0 &&
			!recipient.startsWith('x25519:')
		)
			return true;
	}
	return false;
}

/**
 * Parse the server's latest signed access policy from an access-state response.
 *
 * Returns `null` when no policy exists yet. A present policy must match the
 * browser's {@link AccessPolicy} shape closely enough that rebuilding and
 * re-signing it preserves the server's member, grant, document, and envelope
 * fields; an unexpected shape returns `null` so provisioning is skipped rather
 * than fabricating a policy.
 */
export function parseAccessPolicy(value: unknown): AccessPolicy | null {
	if (value === null || value === undefined) return null;
	if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
	const policy = value as Record<string, unknown>;
	const version = policy.version;
	if (version !== 1 && version !== 2) return null;
	if (
		typeof policy.workspaceId !== 'string' ||
		typeof policy.revision !== 'string' ||
		(policy.previousPolicyDigest !== null &&
			typeof policy.previousPolicyDigest !== 'string') ||
		typeof policy.deviceId !== 'string' ||
		typeof policy.signature !== 'string' ||
		!Array.isArray(policy.members) ||
		!Array.isArray(policy.objects)
	)
		return null;
	const members: AccessPolicy['members'] = [];
	for (const raw of policy.members) {
		if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
		const member = raw as Record<string, unknown>;
		if (
			typeof member.accountId !== 'string' ||
			(member.role !== 'owner' &&
				member.role !== 'admin' &&
				member.role !== 'editor' &&
				member.role !== 'viewer')
		)
			return null;
		members.push({
			accountId: member.accountId,
			role: member.role as AccessPolicy['members'][number]['role'],
		});
	}
	const objects: AccessPolicyObject[] = [];
	for (const raw of policy.objects) {
		if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null;
		const object = raw as Record<string, unknown>;
		if (
			typeof object.objectId !== 'string' ||
			!Number.isSafeInteger(object.epoch) ||
			(object.epoch as number) < 1 ||
			!Array.isArray(object.grants) ||
			!Array.isArray(object.envelopes)
		)
			return null;
		const grants: AccessPolicyObject['grants'] = [];
		for (const rawGrant of object.grants) {
			if (!rawGrant || typeof rawGrant !== 'object' || Array.isArray(rawGrant))
				return null;
			const grant = rawGrant as Record<string, unknown>;
			if (
				typeof grant.accountId !== 'string' ||
				(grant.role !== 'editor' && grant.role !== 'viewer')
			)
				return null;
			grants.push({
				accountId: grant.accountId,
				role: grant.role as AccessPolicyObject['grants'][number]['role'],
			});
		}
		const envelopes: AccessPolicyEnvelope[] = [];
		for (const rawEnvelope of object.envelopes) {
			if (
				!rawEnvelope ||
				typeof rawEnvelope !== 'object' ||
				Array.isArray(rawEnvelope)
			)
				return null;
			const envelope = rawEnvelope as Record<string, unknown>;
			if (
				typeof envelope.deviceId !== 'string' ||
				typeof envelope.wrappedKey !== 'string' ||
				typeof envelope.signature !== 'string'
			)
				return null;
			const parsed: AccessPolicyEnvelope = {
				deviceId: envelope.deviceId,
				wrappedKey: envelope.wrappedKey,
				signature: envelope.signature,
			};
			if (envelope.construction === 'web' || envelope.construction === 'age')
				parsed.construction = envelope.construction;
			if (typeof envelope.recipientPublicKey === 'string')
				parsed.recipientPublicKey = envelope.recipientPublicKey;
			if (typeof envelope.ephemeralPublicKey === 'string')
				parsed.ephemeralPublicKey = envelope.ephemeralPublicKey;
			if (typeof envelope.salt === 'string') parsed.salt = envelope.salt;
			if (typeof envelope.nonce === 'string') parsed.nonce = envelope.nonce;
			envelopes.push(parsed);
		}
		let document: AccessPolicyObject['document'];
		if (
			object.document &&
			typeof object.document === 'object' &&
			!Array.isArray(object.document)
		) {
			const descriptor = object.document as Record<string, unknown>;
			if (
				typeof descriptor.generation === 'string' &&
				(descriptor.mode === 'text' || descriptor.mode === 'attachment')
			)
				document = {
					generation: descriptor.generation,
					mode: descriptor.mode,
				};
		}
		// A version-2 policy binds a document descriptor per object; the server
		// rejects a version-2 object without one. A malformed or partial response
		// must not be rebuilt and re-signed, so skip provisioning instead.
		if (version === 2 && document === undefined) return null;
		objects.push({
			objectId: object.objectId,
			epoch: object.epoch as number,
			grants,
			envelopes,
			...(document === undefined ? {} : { document }),
		});
	}
	return {
		version,
		workspaceId: policy.workspaceId,
		revision: policy.revision,
		previousPolicyDigest: policy.previousPolicyDigest as string | null,
		deviceId: policy.deviceId,
		members,
		objects,
		signature: policy.signature,
	};
}
