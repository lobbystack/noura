/**
 * Platform-independent browser key envelopes for Noura encrypted synchronization.
 *
 * This module implements the portable `noura.sync.key.web` version 1 envelope:
 * X25519 ECDH (RFC 7748), HKDF-SHA256, AES-256-GCM, and Ed25519. It mirrors the
 * Rust `sync-key-envelope` crate byte-for-byte and performs no randomness
 * generation, so callers supply the ephemeral secret, salt, and nonce.
 *
 * Managed synchronization stays end-to-end encrypted. This module only produces
 * and consumes signed ciphertext envelopes and never logs key material.
 *
 * Zeroization is not guaranteed in JavaScript; this module clears references but
 * does not claim reliable memory erasure.
 */

import { blake3 } from '@noble/hashes/blake3.js';

/** Domain string separating browser key envelopes from other signed objects. */
export const DOMAIN = 'noura.sync.key.web';

/** Envelope construction version. */
export const VERSION = 1;

/** HKDF-Expand `info` parameter binding the derived key-encryption key. */
export const INFO = new TextEncoder().encode('noura.sync.key.web.v1');

/** Domain string binding the wrapped object-key plaintext. */
export const OBJECT_KEY_DOMAIN = 'noura.sync.object-key';

/** Prefix identifying a raw 32-byte X25519 browser device recipient. */
export const RECIPIENT_PREFIX = 'x25519:';

/** Domain string separating browser device fingerprints from native ones. */
export const DEVICE_FINGERPRINT_DOMAIN = 'noura.device.card.web';

/** Domain string binding the browser device enrollment proof. */
export const ENROLLMENT_DOMAIN = 'noura.device.enroll.web';

/** Largest accepted positive safe integer epoch (`2^53 - 1`). */
export const MAX_EPOCH = 9007199254740991;

/** Largest accepted wrapped-key ciphertext, bounding allocations from untrusted input. */
const MAX_WRAPPED_KEY = 4096;

/**
 * Byte buffers backed by `ArrayBuffer`, matching the WebCrypto `BufferSource`
 * contract. The global `Uint8Array` default is `ArrayBufferLike`, which the DOM
 * type definitions reject at the `crypto.subtle` boundary.
 */
type Bytes = Uint8Array<ArrayBuffer>;

/**
 * Stable public error codes for adapters, diagnostics, and conformance fixtures.
 *
 * The string values match the Rust `KeyEnvelopeError::code` output exactly.
 */
export const KeyEnvelopeErrorCode = {
	InvalidEnvelope: 'sync_invalid_key_envelope',
	InvalidSignature: 'sync_invalid_signature',
	UnwrapFailed: 'sync_key_unwrap_failed',
	InvalidEpoch: 'sync_invalid_epoch',
	SerializeFailed: 'sync_serialize_failed',
	InvalidBase64: 'sync_invalid_base64',
	InvalidKey: 'sync_invalid_key',
	WrapFailed: 'sync_key_wrap_failed',
	InvalidWrappedKey: 'sync_invalid_wrapped_key',
	InvalidRecipient: 'sync_invalid_recipient',
} as const;

/** Union of the stable browser key envelope error codes. */
export type KeyEnvelopeErrorCode =
	(typeof KeyEnvelopeErrorCode)[keyof typeof KeyEnvelopeErrorCode];

/** Structured failures for browser key envelope handling. */
export class KeyEnvelopeError extends Error {
	readonly code: KeyEnvelopeErrorCode;

	constructor(code: KeyEnvelopeErrorCode, message?: string) {
		super(message ?? code);
		this.name = 'KeyEnvelopeError';
		this.code = code;
	}
}

/**
 * A signed, recipient-wrapped object key for a browser device.
 *
 * Field names are snake_case on the wire, matching native envelope storage. All
 * byte fields are standard (padded) base64.
 */
export interface WebKeyEnvelope {
	/** Stable workspace identifier the object key belongs to. */
	workspace_id: string;
	/** Stable object identifier the key decrypts. */
	object_id: string;
	/** Positive safe integer key epoch. */
	epoch: number;
	/** Device identifier of the signer. */
	signing_device: string;
	/** Device identifier of the intended recipient. */
	device_id: string;
	/** Base64 X25519 recipient public key (32 bytes). */
	recipient_public_key: string;
	/** Base64 X25519 ephemeral public key (32 bytes). */
	ephemeral_public_key: string;
	/** Base64 HKDF salt (32 bytes). */
	salt: string;
	/** Base64 AES-256-GCM nonce (12 bytes). */
	nonce: string;
	/** Base64 AES-256-GCM ciphertext and 16-byte tag. */
	wrapped_key: string;
	/** Base64 Ed25519 signature (64 bytes). */
	signature: string;
}

/** Caller-supplied base64 inputs for {@link wrapKey}. */
export interface WebKeyWrapInputs {
	/** Stable workspace identifier the object key belongs to. */
	workspace_id: string;
	/** Stable object identifier the key decrypts. */
	object_id: string;
	/** Positive safe integer key epoch. */
	epoch: number;
	/** Device identifier of the signer. */
	signing_device: string;
	/** Device identifier of the intended recipient. */
	device_id: string;
	/** Base64 Ed25519 signing seed (32 bytes). */
	signing_secret: string;
	/** Base64 X25519 recipient public key (32 bytes). */
	recipient_public: string;
	/** Base64 object key to wrap (32 bytes). */
	object_key: string;
	/** Base64 X25519 ephemeral secret (32 bytes). */
	ephemeral_secret: string;
	/** Base64 HKDF salt (32 bytes). */
	salt: string;
	/** Base64 AES-256-GCM nonce (12 bytes). */
	nonce: string;
}

/** Caller-supplied inputs for {@link enrollmentProof}. */
export interface WebEnrollmentProofInputs {
	/** Browser origin the proof is bound to. */
	origin: string;
	/** Account identifier the device enrolls under. */
	account_id: string;
	/** Device identifier being enrolled. */
	device_id: string;
	/** Base64 Ed25519 signing seed (32 bytes). */
	signing_secret: string;
	/** Base64 Ed25519 signing public key (32 bytes). */
	signing_public: string;
	/** Browser device recipient string. */
	recipient: string;
	/** Server-issued device challenge. */
	challenge: string;
}

const utf8 = new TextEncoder();

const X25519_PKCS8_PREFIX = Uint8Array.from([
	0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04,
	0x22, 0x04, 0x20,
]);

const ED25519_PKCS8_PREFIX = Uint8Array.from([
	0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04,
	0x22, 0x04, 0x20,
]);

function fail(code: KeyEnvelopeErrorCode, message?: string): never {
	throw new KeyEnvelopeError(code, message);
}

/**
 * Canonical JSON tuple encoding.
 *
 * `JSON.stringify` on an array of strings and plain integers produces the same
 * bytes as Rust `serde_json::to_vec` for these inputs: no whitespace, integers
 * as plain numbers, and standard JSON string escaping. It never pretty-prints.
 */
function canonicalTuple(values: (string | number)[]): Bytes {
	try {
		return utf8.encode(JSON.stringify(values));
	} catch {
		return fail(KeyEnvelopeErrorCode.SerializeFailed);
	}
}

function encodeBase64(bytes: Uint8Array): string {
	let binary = '';
	for (let index = 0; index < bytes.length; index += 1) {
		binary += String.fromCharCode(bytes[index]!);
	}
	return btoa(binary);
}

/**
 * Decode canonical padded base64 within an inclusive byte-length range.
 *
 * Rejects non-canonical encodings (unpadded or trailing-bit variants) and
 * values whose decoded length falls outside `[min, max]`.
 */
function decodeBase64Range(value: string, min: number, max: number): Bytes {
	if (value.length > Math.ceil(max / 3) * 4) {
		return fail(KeyEnvelopeErrorCode.InvalidBase64);
	}
	let binary: string;
	try {
		binary = atob(value);
	} catch {
		return fail(KeyEnvelopeErrorCode.InvalidBase64);
	}
	const bytes = new Uint8Array(binary.length);
	for (let index = 0; index < binary.length; index += 1) {
		bytes[index] = binary.charCodeAt(index);
	}
	if (
		bytes.length < min ||
		bytes.length > max ||
		encodeBase64(bytes) !== value
	) {
		return fail(KeyEnvelopeErrorCode.InvalidBase64);
	}
	return bytes;
}

/** Decode a base64 field that must contain exactly `length` bytes. */
function decodeBase64Fixed(value: string, length: number): Bytes {
	return decodeBase64Range(value, length, length);
}

function concat(prefix: Bytes, suffix: Bytes): Bytes {
	const output = new Uint8Array(prefix.length + suffix.length);
	output.set(prefix, 0);
	output.set(suffix, prefix.length);
	return output;
}

function isAllZero(bytes: Bytes): boolean {
	let accumulator = 0;
	for (const byte of bytes) {
		accumulator |= byte;
	}
	return accumulator === 0;
}

function validateEpoch(epoch: number): void {
	if (!Number.isInteger(epoch) || epoch < 0) {
		return fail(KeyEnvelopeErrorCode.InvalidEnvelope);
	}
	if (epoch === 0 || epoch > MAX_EPOCH) {
		return fail(KeyEnvelopeErrorCode.InvalidEpoch);
	}
}

const ENVELOPE_STRING_FIELDS = [
	'workspace_id',
	'object_id',
	'signing_device',
	'device_id',
	'recipient_public_key',
	'ephemeral_public_key',
	'salt',
	'nonce',
	'wrapped_key',
	'signature',
] as const;

function validateEnvelopeShape(envelope: WebKeyEnvelope): void {
	if (typeof envelope !== 'object' || envelope === null) {
		return fail(KeyEnvelopeErrorCode.InvalidEnvelope);
	}
	for (const field of ENVELOPE_STRING_FIELDS) {
		if (typeof envelope[field] !== 'string') {
			return fail(KeyEnvelopeErrorCode.InvalidEnvelope);
		}
	}
	if (typeof envelope.epoch !== 'number' || !Number.isInteger(envelope.epoch)) {
		return fail(KeyEnvelopeErrorCode.InvalidEnvelope);
	}
	if (envelope.epoch < 0) {
		return fail(KeyEnvelopeErrorCode.InvalidEnvelope);
	}
}

/** Canonical JSON additional authenticated data binding routing metadata. */
function envelopeAad(envelope: WebKeyEnvelope): Bytes {
	return canonicalTuple([
		DOMAIN,
		VERSION,
		envelope.workspace_id,
		envelope.object_id,
		envelope.epoch,
		envelope.signing_device,
		envelope.device_id,
		envelope.recipient_public_key,
		envelope.ephemeral_public_key,
		envelope.salt,
		envelope.nonce,
	]);
}

/** Canonical JSON signing tuple covering the complete envelope. */
function envelopeSigningBytes(envelope: WebKeyEnvelope): Bytes {
	return canonicalTuple([
		DOMAIN,
		VERSION,
		envelope.workspace_id,
		envelope.object_id,
		envelope.epoch,
		envelope.signing_device,
		envelope.device_id,
		envelope.recipient_public_key,
		envelope.ephemeral_public_key,
		envelope.salt,
		envelope.nonce,
		envelope.wrapped_key,
	]);
}

async function importX25519PrivateKey(
	secret: Bytes,
	usages: KeyUsage[],
): Promise<CryptoKey> {
	return crypto.subtle.importKey(
		'pkcs8',
		concat(X25519_PKCS8_PREFIX, secret),
		{ name: 'X25519' },
		false,
		usages,
	);
}

async function x25519PublicFromSecret(secret: Bytes): Promise<Bytes> {
	const privateKey = await importX25519PrivateKey(secret, ['deriveBits']);
	const basepoint = new Uint8Array(32);
	basepoint[0] = 9;
	const publicKey = await crypto.subtle.importKey(
		'raw',
		basepoint,
		{ name: 'X25519' },
		false,
		[],
	);
	const bits = await crypto.subtle.deriveBits(
		{ name: 'X25519', public: publicKey },
		privateKey,
		256,
	);
	return new Uint8Array(bits);
}

async function x25519SharedSecret(
	secret: Bytes,
	publicKeyBytes: Bytes,
): Promise<Bytes> {
	const privateKey = await importX25519PrivateKey(secret, ['deriveBits']);
	const publicKey = await crypto.subtle.importKey(
		'raw',
		publicKeyBytes,
		{ name: 'X25519' },
		false,
		[],
	);
	const bits = await crypto.subtle.deriveBits(
		{ name: 'X25519', public: publicKey },
		privateKey,
		256,
	);
	return new Uint8Array(bits);
}

/** Derive the AES-256-GCM key with HKDF-SHA256 over the ECDH shared secret. */
async function deriveKey(salt: Bytes, shared: Bytes): Promise<Bytes> {
	const hkdfKey = await crypto.subtle.importKey('raw', shared, 'HKDF', false, [
		'deriveBits',
	]);
	const bits = await crypto.subtle.deriveBits(
		{ name: 'HKDF', hash: 'SHA-256', salt, info: INFO },
		hkdfKey,
		256,
	);
	return new Uint8Array(bits);
}

async function aesEncrypt(
	key: Bytes,
	nonce: Bytes,
	additionalData: Bytes,
	plaintext: Bytes,
): Promise<Bytes> {
	const aesKey = await crypto.subtle.importKey('raw', key, 'AES-GCM', false, [
		'encrypt',
	]);
	const ciphertext = await crypto.subtle.encrypt(
		{ name: 'AES-GCM', iv: nonce, additionalData, tagLength: 128 },
		aesKey,
		plaintext,
	);
	return new Uint8Array(ciphertext);
}

async function aesDecrypt(
	key: Bytes,
	nonce: Bytes,
	additionalData: Bytes,
	ciphertext: Bytes,
): Promise<Bytes> {
	const aesKey = await crypto.subtle.importKey('raw', key, 'AES-GCM', false, [
		'decrypt',
	]);
	const plaintext = await crypto.subtle.decrypt(
		{ name: 'AES-GCM', iv: nonce, additionalData, tagLength: 128 },
		aesKey,
		ciphertext,
	);
	return new Uint8Array(plaintext);
}

async function ed25519Sign(secret: Bytes, message: Bytes): Promise<Bytes> {
	const key = await crypto.subtle.importKey(
		'pkcs8',
		concat(ED25519_PKCS8_PREFIX, secret),
		{ name: 'Ed25519' },
		false,
		['sign'],
	);
	const signature = await crypto.subtle.sign({ name: 'Ed25519' }, key, message);
	return new Uint8Array(signature);
}

async function ed25519Verify(
	publicKey: Bytes,
	signature: Bytes,
	message: Bytes,
): Promise<void> {
	let key: CryptoKey;
	try {
		key = await crypto.subtle.importKey(
			'raw',
			publicKey,
			{ name: 'Ed25519' },
			false,
			['verify'],
		);
	} catch {
		return fail(KeyEnvelopeErrorCode.InvalidKey);
	}
	let valid: boolean;
	try {
		valid = await crypto.subtle.verify(
			{ name: 'Ed25519' },
			key,
			signature,
			message,
		);
	} catch {
		return fail(KeyEnvelopeErrorCode.InvalidSignature);
	}
	if (!valid) {
		return fail(KeyEnvelopeErrorCode.InvalidSignature);
	}
}

/**
 * Wrap an object key to a browser device's X25519 public key and sign it.
 *
 * The caller supplies the ephemeral secret, HKDF salt, and AES-GCM nonce so this
 * module performs no randomness generation and stays portable.
 */
export async function wrapKey(
	inputs: WebKeyWrapInputs,
): Promise<WebKeyEnvelope> {
	validateEpoch(inputs.epoch);
	const signingSecret = decodeBase64Fixed(inputs.signing_secret, 32);
	const recipientPublic = decodeBase64Fixed(inputs.recipient_public, 32);
	const objectKey = decodeBase64Fixed(inputs.object_key, 32);
	const ephemeralSecret = decodeBase64Fixed(inputs.ephemeral_secret, 32);
	const salt = decodeBase64Fixed(inputs.salt, 32);
	const nonce = decodeBase64Fixed(inputs.nonce, 12);

	let shared: Bytes;
	try {
		shared = await x25519SharedSecret(ephemeralSecret, recipientPublic);
	} catch {
		return fail(KeyEnvelopeErrorCode.InvalidEnvelope);
	}
	if (isAllZero(shared)) {
		return fail(KeyEnvelopeErrorCode.InvalidEnvelope);
	}
	const key = await deriveKey(salt, shared);

	const envelope: WebKeyEnvelope = {
		workspace_id: inputs.workspace_id,
		object_id: inputs.object_id,
		epoch: inputs.epoch,
		signing_device: inputs.signing_device,
		device_id: inputs.device_id,
		recipient_public_key: encodeBase64(recipientPublic),
		ephemeral_public_key: '',
		salt: encodeBase64(salt),
		nonce: encodeBase64(nonce),
		wrapped_key: '',
		signature: '',
	};

	try {
		envelope.ephemeral_public_key = encodeBase64(
			await x25519PublicFromSecret(ephemeralSecret),
		);
	} catch {
		return fail(KeyEnvelopeErrorCode.InvalidEnvelope);
	}

	const plaintext = canonicalTuple([
		OBJECT_KEY_DOMAIN,
		VERSION,
		inputs.workspace_id,
		inputs.object_id,
		inputs.epoch,
		encodeBase64(objectKey),
	]);
	try {
		envelope.wrapped_key = encodeBase64(
			await aesEncrypt(key, nonce, envelopeAad(envelope), plaintext),
		);
	} catch {
		return fail(KeyEnvelopeErrorCode.WrapFailed);
	}

	envelope.signature = encodeBase64(
		await ed25519Sign(signingSecret, envelopeSigningBytes(envelope)),
	);
	return envelope;
}

/** Verify an envelope's signature against a previously trusted signer key. */
export async function verifyEnvelope(
	envelope: WebKeyEnvelope,
	trustedSignerPublic: Bytes,
): Promise<void> {
	validateEnvelopeShape(envelope);
	validateEpoch(envelope.epoch);
	decodeBase64Fixed(envelope.recipient_public_key, 32);
	decodeBase64Fixed(envelope.ephemeral_public_key, 32);
	decodeBase64Fixed(envelope.salt, 32);
	decodeBase64Fixed(envelope.nonce, 12);
	decodeBase64Range(envelope.wrapped_key, 16, MAX_WRAPPED_KEY);
	const signature = decodeBase64Fixed(envelope.signature, 64);
	await ed25519Verify(
		trustedSignerPublic,
		signature,
		envelopeSigningBytes(envelope),
	);
}

/** Verify then unwrap an object key. Verification always precedes decryption. */
export async function unwrapKey(
	envelope: WebKeyEnvelope,
	recipientSecret: Bytes,
	trustedSignerPublic: Bytes,
): Promise<Bytes> {
	await verifyEnvelope(envelope, trustedSignerPublic);

	const ephemeralPublic = decodeBase64Fixed(envelope.ephemeral_public_key, 32);
	const salt = decodeBase64Fixed(envelope.salt, 32);
	const nonce = decodeBase64Fixed(envelope.nonce, 12);
	const ciphertext = decodeBase64Range(
		envelope.wrapped_key,
		16,
		MAX_WRAPPED_KEY,
	);

	let shared: Bytes;
	try {
		shared = await x25519SharedSecret(recipientSecret, ephemeralPublic);
	} catch {
		return fail(KeyEnvelopeErrorCode.UnwrapFailed);
	}
	if (isAllZero(shared)) {
		return fail(KeyEnvelopeErrorCode.UnwrapFailed);
	}
	const key = await deriveKey(salt, shared);

	let plaintext: Bytes;
	try {
		plaintext = await aesDecrypt(key, nonce, envelopeAad(envelope), ciphertext);
	} catch {
		return fail(KeyEnvelopeErrorCode.UnwrapFailed);
	}

	let parsed: unknown;
	try {
		const decoded = new TextDecoder('utf-8', { fatal: true }).decode(plaintext);
		parsed = JSON.parse(decoded);
	} catch {
		return fail(KeyEnvelopeErrorCode.InvalidWrappedKey);
	}
	if (!Array.isArray(parsed) || parsed.length !== 6) {
		return fail(KeyEnvelopeErrorCode.InvalidWrappedKey);
	}
	const [domain, version, workspaceId, objectId, epoch, encoded] = parsed;
	if (
		domain !== OBJECT_KEY_DOMAIN ||
		version !== VERSION ||
		workspaceId !== envelope.workspace_id ||
		objectId !== envelope.object_id ||
		epoch !== envelope.epoch
	) {
		return fail(KeyEnvelopeErrorCode.InvalidWrappedKey);
	}
	if (typeof encoded !== 'string') {
		return fail(KeyEnvelopeErrorCode.InvalidWrappedKey);
	}
	return decodeBase64Fixed(encoded, 32);
}

/** Render bytes as lowercase hexadecimal. */
function bytesToHex(bytes: Uint8Array): string {
	let output = '';
	for (const byte of bytes) {
		output += byte.toString(16).padStart(2, '0');
	}
	return output;
}

/**
 * Encode a raw X25519 public key as a browser device recipient string.
 *
 * The encoding is `x25519:` followed by standard (padded) base64 of the 32-byte
 * public key. It is the browser counterpart to an `age` recipient and is bound
 * into the browser device fingerprint.
 */
export function encodeRecipient(publicKey: Uint8Array): string {
	return `${RECIPIENT_PREFIX}${encodeBase64(publicKey)}`;
}

/**
 * Decode a browser device recipient string into its raw X25519 public key.
 *
 * Only `x25519:` followed by canonical standard base64 of exactly 32 bytes is
 * accepted. Every other form (a missing prefix, an `age1` recipient, a
 * non-canonical encoding, or the wrong decoded length) is rejected with
 * {@link KeyEnvelopeErrorCode.InvalidRecipient}.
 */
export function decodeRecipient(recipient: string): Bytes {
	if (!recipient.startsWith(RECIPIENT_PREFIX)) {
		return fail(KeyEnvelopeErrorCode.InvalidRecipient);
	}
	const encoded = recipient.slice(RECIPIENT_PREFIX.length);
	if (encoded.length !== 44) {
		return fail(KeyEnvelopeErrorCode.InvalidRecipient);
	}
	let binary: string;
	try {
		binary = atob(encoded);
	} catch {
		return fail(KeyEnvelopeErrorCode.InvalidRecipient);
	}
	const bytes = new Uint8Array(binary.length);
	for (let index = 0; index < binary.length; index += 1) {
		bytes[index] = binary.charCodeAt(index);
	}
	if (bytes.length !== 32 || encodeBase64(bytes) !== encoded) {
		return fail(KeyEnvelopeErrorCode.InvalidRecipient);
	}
	return bytes;
}

/**
 * Compute the browser device fingerprint for a raw X25519 recipient.
 *
 * The fingerprint is lowercase hexadecimal BLAKE3 over the canonical JSON tuple
 * `[DEVICE_FINGERPRINT_DOMAIN, 1, deviceId, accountId, base64(signingPublic),
 * recipient]`. It mirrors the native `noura.device.card` fingerprint, differing
 * only in the domain string and the raw X25519 recipient representation. The
 * recipient is bound as given; call {@link decodeRecipient} first when it must
 * be validated.
 */
export function deviceFingerprint(
	deviceId: string,
	accountId: string,
	signingPublic: Uint8Array,
	recipient: string,
): string {
	const bytes = canonicalTuple([
		DEVICE_FINGERPRINT_DOMAIN,
		1,
		deviceId,
		accountId,
		encodeBase64(signingPublic),
		recipient,
	]);
	return bytesToHex(blake3(bytes));
}

/**
 * Sign the browser device enrollment tuple and return the raw signature.
 *
 * The signed message is the canonical JSON tuple
 * `[ENROLLMENT_DOMAIN, 1, origin, accountId, deviceId, base64(signingPublic),
 * recipient, challenge]`. `signing_secret` is the Ed25519 signing seed and
 * `signing_public` its verifying key, included so the server can bind the public
 * key exactly as native enrollment does. The recipient is bound as given; call
 * {@link decodeRecipient} first when it must be validated.
 */
export async function enrollmentProof(
	inputs: WebEnrollmentProofInputs,
): Promise<Bytes> {
	const signingSecret = decodeBase64Fixed(inputs.signing_secret, 32);
	const signingPublic = decodeBase64Fixed(inputs.signing_public, 32);
	const message = canonicalTuple([
		ENROLLMENT_DOMAIN,
		1,
		inputs.origin,
		inputs.account_id,
		inputs.device_id,
		encodeBase64(signingPublic),
		inputs.recipient,
		inputs.challenge,
	]);
	return ed25519Sign(signingSecret, message);
}
