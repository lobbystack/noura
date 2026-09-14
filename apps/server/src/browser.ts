import { createPublicKey, verify } from 'node:crypto';
import { base64, identifier, record, SyncError } from './protocol';

/**
 * Server-side helpers for the `noura.sync.key.web` browser key envelope and the
 * `noura.device.enroll.web` browser enrollment proof.
 *
 * Only Ed25519 verification and byte-length validation happen here. The server
 * never unwraps a recipient key; it stores signed ciphertext and validates
 * authorization exactly as it does for native `age` envelopes.
 */

/** Domain string separating browser key envelopes from native ones. */
export const WEB_KEY_DOMAIN = 'noura.sync.key.web';

/** Domain string binding the browser device enrollment proof. */
export const WEB_ENROLLMENT_DOMAIN = 'noura.device.enroll.web';

/** Prefix identifying a raw 32-byte X25519 browser device recipient. */
export const RECIPIENT_PREFIX = 'x25519:';

const AGE_RECIPIENT = /^age1[023456789acdefghjklmnpqrstuvwxyz]{58}$/;
const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

/** True for the native Bech32 recipient form accepted during enrollment. */
export function isAgeRecipient(value: unknown): value is string {
	return typeof value === 'string' && AGE_RECIPIENT.test(value);
}

/**
 * Decode a browser device recipient: `x25519:` followed by canonical padded
 * base64 of exactly 32 bytes. Every other form is rejected.
 */
export function decodeBrowserRecipient(value: string): Buffer {
	if (!value.startsWith(RECIPIENT_PREFIX))
		throw new SyncError('sync.invalid_recipient');
	const encoded = value.slice(RECIPIENT_PREFIX.length);
	if (encoded.length !== 44) throw new SyncError('sync.invalid_recipient');
	const decoded = Buffer.from(encoded, 'base64');
	if (decoded.length !== 32 || decoded.toString('base64') !== encoded)
		throw new SyncError('sync.invalid_recipient');
	return decoded;
}

/** Decode a browser recipient, returning `undefined` instead of throwing. */
export function browserRecipient(value: unknown): Buffer | undefined {
	if (typeof value !== 'string') return undefined;
	try {
		return decodeBrowserRecipient(value);
	} catch {
		return undefined;
	}
}

/** True when a stored recipient string spells the same 32 X25519 bytes. */
export function browserRecipientMatches(
	stored: unknown,
	recipientPublicKey: string,
): boolean {
	const decoded = browserRecipient(stored);
	if (!decoded) return false;
	return decoded.equals(Buffer.from(recipientPublicKey, 'base64'));
}

/** Verify an Ed25519 signature, returning `false` for malformed keys. */
export function verifyEd25519(
	publicKey: string,
	message: Buffer,
	signature: string,
): boolean {
	try {
		const key = createPublicKey({
			key: Buffer.concat([
				ED25519_SPKI_PREFIX,
				Buffer.from(publicKey, 'base64'),
			]),
			format: 'der',
			type: 'spki',
		});
		return verify(null, message, key, Buffer.from(signature, 'base64'));
	} catch {
		return false;
	}
}

/**
 * Canonical `noura.device.enroll.web` version 1 signing tuple.
 *
 * `["noura.device.enroll.web",1,origin,accountId,deviceId,base64(publicKey),recipient,challenge]`
 */
export function browserEnrollmentSigningBytes(input: {
	origin: string;
	accountId: string;
	deviceId: string;
	publicKey: string;
	recipient: string;
	challenge: string;
}): Buffer {
	return Buffer.from(
		JSON.stringify([
			WEB_ENROLLMENT_DOMAIN,
			1,
			input.origin,
			input.accountId,
			input.deviceId,
			input.publicKey,
			input.recipient,
			input.challenge,
		]),
	);
}

/** Fields bound by the `noura.sync.key.web` version 1 signing tuple. */
export interface BrowserKeyTupleFields {
	workspaceId: string;
	objectId: string;
	epoch: number;
	signingDevice: string;
	deviceId: string;
	recipientPublicKey: string;
	ephemeralPublicKey: string;
	salt: string;
	nonce: string;
	wrappedKey: string;
}

/**
 * Canonical `noura.sync.key.web` version 1 signing tuple, byte-for-byte equal to
 * the Rust `WebKeyEnvelope::signing_bytes`.
 */
export function browserKeySigningBytes(fields: BrowserKeyTupleFields): Buffer {
	return Buffer.from(
		JSON.stringify([
			WEB_KEY_DOMAIN,
			1,
			fields.workspaceId,
			fields.objectId,
			fields.epoch,
			fields.signingDevice,
			fields.deviceId,
			fields.recipientPublicKey,
			fields.ephemeralPublicKey,
			fields.salt,
			fields.nonce,
			fields.wrappedKey,
		]),
	);
}

/** A parsed `PUT /v1/keys/self` envelope normalized to one storage shape. */
export interface StoredKeyEnvelope {
	construction: 'age' | 'web';
	workspaceId: string;
	objectId: string;
	epoch: number;
	deviceId: string;
	wrappedKey: string;
	signingDevice: string;
	signature: string;
	recipientPublicKey: string | null;
	ephemeralPublicKey: string | null;
	salt: string | null;
	nonce: string | null;
}

const AGE_ENVELOPE_FIELDS = [
	'workspaceId',
	'objectId',
	'epoch',
	'deviceId',
	'wrappedKey',
	'signingDevice',
	'signature',
];
const WEB_ENVELOPE_FIELDS = [
	...AGE_ENVELOPE_FIELDS,
	'construction',
	'recipientPublicKey',
	'ephemeralPublicKey',
	'salt',
	'nonce',
];

function exact(value: Record<string, unknown>, fields: string[]) {
	if (
		Object.keys(value).length !== fields.length ||
		fields.some((field) => !(field in value))
	)
		throw new SyncError('sync.invalid_key');
}

/**
 * Parse a key-envelope DTO. Absent `construction` means `age`; `construction:
 * "web"` additionally requires the four browser byte fields. Unknown
 * constructions and mixed or missing fields are rejected.
 */
export function parseKeyEnvelope(input: unknown): StoredKeyEnvelope {
	const value = record(input);
	const construction = value.construction ?? 'age';
	if (construction !== 'age' && construction !== 'web')
		throw new SyncError('sync.invalid_key');
	if (construction === 'web') {
		exact(value, WEB_ENVELOPE_FIELDS);
	} else {
		exact(
			value,
			value.construction === undefined
				? AGE_ENVELOPE_FIELDS
				: [...AGE_ENVELOPE_FIELDS, 'construction'],
		);
	}
	if (!Number.isSafeInteger(value.epoch) || (value.epoch as number) < 1)
		throw new SyncError('sync.invalid_epoch');
	const common = {
		workspaceId: identifier(value.workspaceId),
		objectId: identifier(value.objectId),
		epoch: value.epoch as number,
		deviceId: identifier(value.deviceId),
		signingDevice: identifier(value.signingDevice),
		wrappedKey: value.wrappedKey as string,
		signature: base64(value.signature, 64).toString('base64'),
	};
	if (construction === 'web') {
		base64(value.wrappedKey, 16, 4096);
		return {
			...common,
			construction: 'web',
			recipientPublicKey: base64(value.recipientPublicKey, 32).toString(
				'base64',
			),
			ephemeralPublicKey: base64(value.ephemeralPublicKey, 32).toString(
				'base64',
			),
			salt: base64(value.salt, 32).toString('base64'),
			nonce: base64(value.nonce, 12).toString('base64'),
		};
	}
	base64(value.wrappedKey, 60, 4096);
	return {
		...common,
		construction: 'age',
		recipientPublicKey: null,
		ephemeralPublicKey: null,
		salt: null,
		nonce: null,
	};
}

/**
 * Verify a parsed envelope's signature against a previously trusted Ed25519
 * public key. Native envelopes use `noura.sync.key`; browser envelopes use
 * `noura.sync.key.web`.
 */
export function verifyKeyEnvelopeSignature(
	value: StoredKeyEnvelope,
	publicKey: string,
): boolean {
	const message =
		value.construction === 'web'
			? browserKeySigningBytes({
					workspaceId: value.workspaceId,
					objectId: value.objectId,
					epoch: value.epoch,
					signingDevice: value.signingDevice,
					deviceId: value.deviceId,
					recipientPublicKey: value.recipientPublicKey!,
					ephemeralPublicKey: value.ephemeralPublicKey!,
					salt: value.salt!,
					nonce: value.nonce!,
					wrappedKey: value.wrappedKey,
				})
			: Buffer.from(
					JSON.stringify([
						'noura.sync.key',
						1,
						value.workspaceId,
						value.objectId,
						value.epoch,
						value.signingDevice,
						value.deviceId,
						value.wrappedKey,
					]),
				);
	return verifyEd25519(publicKey, message, value.signature);
}
