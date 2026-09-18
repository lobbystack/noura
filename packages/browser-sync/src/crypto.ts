/**
 * WebCrypto primitives for browser device custody and for the browser sync HTTP
 * contract.
 *
 * All randomness comes from `globalThis.crypto.getRandomValues`; all
 * cryptography uses `globalThis.crypto.subtle`. This module performs no I/O and
 * never logs key material.
 */

import { BrowserSyncError, BrowserSyncErrorCode } from './errors';

/**
 * Byte buffers backed by `ArrayBuffer`, matching the WebCrypto `BufferSource`
 * contract. The global `Uint8Array` default is `ArrayBufferLike`, which the DOM
 * type definitions reject at the `crypto.subtle` boundary.
 */
export type Bytes = Uint8Array<ArrayBuffer>;

/** Minimum PBKDF2-SHA256 iteration count accepted for at-rest key custody. */
export const PBKDF2_MIN_ITERATIONS = 310000;

/** Domain string binding the wrapped at-rest key bundle. */
export const BUNDLE_DOMAIN = 'noura.browser-sync.bundle';

/** Wrapped bundle format version. */
export const BUNDLE_VERSION = 1;

/** Key-derivation function identifier recorded in a wrapped bundle. */
export const BUNDLE_KDF = 'pbkdf2-sha256';

/** Length in bytes of the PBKDF2 salt. */
export const SALT_LENGTH = 32;

/** Length in bytes of the AES-256-GCM nonce. */
export const NONCE_LENGTH = 12;

/** Length in bytes of every Ed25519 seed, X25519 secret, and X25519 public key. */
export const SECRET_LENGTH = 32;

/** AES-GCM authentication tag length in bits. */
export const GCM_TAG_LENGTH = 128;

const ED25519_PKCS8_PREFIX = [
	0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04,
	0x22, 0x04, 0x20,
];

const X25519_PKCS8_PREFIX = [
	0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04,
	0x22, 0x04, 0x20,
];

const utf8 = new TextEncoder();
const utf8Fatal = new TextDecoder('utf-8', { fatal: true });

/**
 * Fixed, domain-separated message signed to confirm a signing seed corresponds
 * to its public key. It is a constant, never caller-controlled, so it is not a
 * signing oracle.
 */
const KEY_CHECK_MESSAGE = utf8.encode('noura.browser-sync.key-check.v1');

/** Fill a new buffer with cryptographically secure random bytes. */
export function randomBytes(length: number): Bytes {
	const output = new Uint8Array(length);
	globalThis.crypto.getRandomValues(output);
	return output;
}

/** Encode bytes as canonical standard (padded) base64. */
export function encodeBase64(bytes: Uint8Array): string {
	let binary = '';
	for (let index = 0; index < bytes.length; index += 1) {
		binary += String.fromCharCode(bytes[index]!);
	}
	return btoa(binary);
}

/**
 * Encode bytes as canonical unpadded base64url.
 *
 * Used for generated device and bundle identifiers, which must match the
 * server's identifier alphabet.
 */
export function encodeBase64Url(bytes: Uint8Array): string {
	return encodeBase64(bytes)
		.replace(/\+/g, '-')
		.replace(/\//g, '_')
		.replace(/=+$/, '');
}

/**
 * Decode canonical standard (padded) base64, optionally requiring an exact
 * decoded length. Non-canonical encodings are rejected.
 */
export function decodeBase64(value: string, expectedLength?: number): Bytes {
	let binary: string;
	try {
		binary = atob(value);
	} catch {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'value was not valid base64',
		);
	}
	const bytes = new Uint8Array(binary.length);
	for (let index = 0; index < binary.length; index += 1) {
		bytes[index] = binary.charCodeAt(index);
	}
	if (encodeBase64(bytes) !== value) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'value was not canonical base64',
		);
	}
	if (expectedLength !== undefined && bytes.length !== expectedLength) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'value had the wrong decoded length',
		);
	}
	return bytes;
}

/** Copy exactly `length` bytes, rejecting any other length. */
export function fixedBytes(
	value: Uint8Array,
	length: number,
	label: string,
): Bytes {
	if (value.length !== length) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			`${label} must be ${length} bytes`,
		);
	}
	const output = new Uint8Array(length);
	output.set(value);
	return output;
}

/** Constant-time-ish byte comparison by length-aware accumulation. */
export function bytesEqual(left: Uint8Array, right: Uint8Array): boolean {
	if (left.length !== right.length) return false;
	let difference = 0;
	for (let index = 0; index < left.length; index += 1) {
		difference |= left[index]! ^ right[index]!;
	}
	return difference === 0;
}

/** True when `value` matches the server's identifier alphabet and length. */
export function isIdentifier(value: unknown): value is string {
	return typeof value === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(value);
}

/** Generate a fresh identifier suitable for a device or bundle key. */
export function randomIdentifier(): string {
	return encodeBase64Url(randomBytes(16));
}

/** Encode a tuple as canonical JSON bytes for signing or authenticated data. */
export function canonicalBytes(values: (string | number)[]): Bytes {
	return utf8.encode(JSON.stringify(values));
}

/** Raw device key material generated by WebCrypto. */
export interface GeneratedDeviceKeys {
	/** Ed25519 signing seed (32 bytes). */
	signingSeed: Bytes;
	/** Ed25519 signing public key (32 bytes). */
	signingPublic: Bytes;
	/** X25519 recipient secret (32 bytes). */
	x25519Secret: Bytes;
	/** X25519 recipient public key (32 bytes). */
	x25519Public: Bytes;
}

function stripPkcs8Prefix(
	pkcs8: Uint8Array,
	prefix: number[],
	label: string,
): Bytes {
	if (pkcs8.length !== prefix.length + SECRET_LENGTH) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidIdentity,
			`${label} had an unexpected encoding`,
		);
	}
	for (let index = 0; index < prefix.length; index += 1) {
		if (pkcs8[index] !== prefix[index]) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidIdentity,
				`${label} had an unexpected encoding`,
			);
		}
	}
	const output = new Uint8Array(SECRET_LENGTH);
	output.set(pkcs8.subarray(prefix.length));
	return output;
}

/** Generate an Ed25519 signing key pair. */
export async function generateSigningKey(): Promise<{
	signingSeed: Bytes;
	signingPublic: Bytes;
}> {
	const pair = (await globalThis.crypto.subtle.generateKey(
		{ name: 'Ed25519' },
		true,
		['sign', 'verify'],
	)) as CryptoKeyPair;
	const pkcs8 = new Uint8Array(
		await globalThis.crypto.subtle.exportKey('pkcs8', pair.privateKey),
	);
	const signingPublic = new Uint8Array(
		await globalThis.crypto.subtle.exportKey('raw', pair.publicKey),
	);
	return {
		signingSeed: stripPkcs8Prefix(pkcs8, ED25519_PKCS8_PREFIX, 'signing key'),
		signingPublic: fixedBytes(signingPublic, SECRET_LENGTH, 'signing public'),
	};
}

/** Generate an X25519 recipient key pair. */
export async function generateRecipientKey(): Promise<{
	x25519Secret: Bytes;
	x25519Public: Bytes;
}> {
	const pair = (await globalThis.crypto.subtle.generateKey(
		{ name: 'X25519' },
		true,
		['deriveBits'],
	)) as CryptoKeyPair;
	const pkcs8 = new Uint8Array(
		await globalThis.crypto.subtle.exportKey('pkcs8', pair.privateKey),
	);
	const x25519Public = new Uint8Array(
		await globalThis.crypto.subtle.exportKey('raw', pair.publicKey),
	);
	return {
		x25519Secret: stripPkcs8Prefix(pkcs8, X25519_PKCS8_PREFIX, 'recipient key'),
		x25519Public: fixedBytes(x25519Public, SECRET_LENGTH, 'recipient public'),
	};
}

/** Generate the Ed25519 signing and X25519 recipient key pairs together. */
export async function generateDeviceKeys(): Promise<GeneratedDeviceKeys> {
	const signing = await generateSigningKey();
	const recipient = await generateRecipientKey();
	return { ...signing, ...recipient };
}

/**
 * Derive an AES-256-GCM key-encryption key from a passphrase with
 * PBKDF2-SHA256.
 *
 * This is the fallback unlock factor. WebAuthn PRF is the preferred factor and
 * is intentionally not implemented here; see `docs/architecture/browser-sync.md`.
 */
export async function deriveKek(
	passphrase: string,
	salt: Bytes,
	iterations: number,
): Promise<CryptoKey> {
	const material = await globalThis.crypto.subtle.importKey(
		'raw',
		utf8.encode(passphrase),
		'PBKDF2',
		false,
		['deriveBits'],
	);
	const bits = await globalThis.crypto.subtle.deriveBits(
		{ name: 'PBKDF2', salt, iterations, hash: 'SHA-256' },
		material,
		256,
	);
	return globalThis.crypto.subtle.importKey(
		'raw',
		new Uint8Array(bits),
		'AES-GCM',
		false,
		['encrypt', 'decrypt'],
	);
}

/** Encrypt with AES-256-GCM, returning ciphertext plus the authentication tag. */
export async function aesGcmEncrypt(
	key: CryptoKey,
	nonce: Bytes,
	additionalData: Bytes,
	plaintext: Bytes,
): Promise<Bytes> {
	const ciphertext = await globalThis.crypto.subtle.encrypt(
		{
			name: 'AES-GCM',
			iv: nonce,
			additionalData,
			tagLength: GCM_TAG_LENGTH,
		},
		key,
		plaintext,
	);
	return new Uint8Array(ciphertext);
}

/** Decrypt AES-256-GCM ciphertext plus tag. Throws on authentication failure. */
export async function aesGcmDecrypt(
	key: CryptoKey,
	nonce: Bytes,
	additionalData: Bytes,
	ciphertext: Bytes,
): Promise<Bytes> {
	const plaintext = await globalThis.crypto.subtle.decrypt(
		{
			name: 'AES-GCM',
			iv: nonce,
			additionalData,
			tagLength: GCM_TAG_LENGTH,
		},
		key,
		ciphertext,
	);
	return new Uint8Array(plaintext);
}

/** Encode bytes as a UTF-8 string using a fatal decoder. */
export function decodeUtf8(bytes: Uint8Array): string {
	return utf8Fatal.decode(bytes);
}

/** Encode a string as UTF-8 bytes. */
export function encodeUtf8(value: string): Bytes {
	return utf8.encode(value);
}

function concatBytes(prefix: readonly number[], suffix: Uint8Array): Bytes {
	const output = new Uint8Array(prefix.length + suffix.length);
	output.set(prefix, 0);
	output.set(suffix, prefix.length);
	return output;
}

/**
 * Import a raw 32-byte Ed25519 signing seed as a non-extractable signing key.
 *
 * The seed is wrapped in the fixed PKCS#8 Ed25519 prefix so WebCrypto can import
 * it. The returned key can only sign; it cannot be exported.
 */
export async function importSigningKey(seed: Uint8Array): Promise<CryptoKey> {
	const pkcs8 = concatBytes(
		ED25519_PKCS8_PREFIX,
		fixedBytes(seed, SECRET_LENGTH, 'signing seed'),
	);
	return globalThis.crypto.subtle.importKey(
		'pkcs8',
		pkcs8,
		{ name: 'Ed25519' },
		false,
		['sign'],
	);
}

/** Import a raw 32-byte Ed25519 public key for verification only. */
export async function importVerifyKey(
	publicKey: Uint8Array,
): Promise<CryptoKey> {
	return globalThis.crypto.subtle.importKey(
		'raw',
		fixedBytes(publicKey, SECRET_LENGTH, 'signing public key'),
		{ name: 'Ed25519' },
		false,
		['verify'],
	);
}

/** Import a raw 32-byte object key as an AES-256-GCM key. */
export async function importAesKey(
	bytes: Uint8Array,
	usages: KeyUsage[],
): Promise<CryptoKey> {
	return globalThis.crypto.subtle.importKey(
		'raw',
		fixedBytes(bytes, SECRET_LENGTH, 'object key'),
		{ name: 'AES-GCM' },
		false,
		usages,
	);
}

/**
 * Derive the raw X25519 public key for a 32-byte recipient secret.
 *
 * This computes `X25519(secret, 9)` using the curve base point, matching
 * `@noura/sync-key-envelope` and the native client. It confirms that a wrapped
 * bundle's stored recipient public key is the one its recipient secret
 * generates.
 */
export async function deriveRecipientPublic(
	secret: Uint8Array,
): Promise<Bytes> {
	const privateKey = await globalThis.crypto.subtle.importKey(
		'pkcs8',
		concatBytes(
			X25519_PKCS8_PREFIX,
			fixedBytes(secret, SECRET_LENGTH, 'recipient secret'),
		),
		{ name: 'X25519' },
		false,
		['deriveBits'],
	);
	const basepoint = new Uint8Array(SECRET_LENGTH);
	basepoint[0] = 9;
	const publicKey = await globalThis.crypto.subtle.importKey(
		'raw',
		basepoint,
		{ name: 'X25519' },
		false,
		[],
	);
	const bits = await globalThis.crypto.subtle.deriveBits(
		{ name: 'X25519', public: publicKey },
		privateKey,
		256,
	);
	return fixedBytes(new Uint8Array(bits), SECRET_LENGTH, 'recipient public');
}

/**
 * True when `seed` is the Ed25519 signing seed for `publicKey`.
 *
 * WebCrypto cannot derive an Ed25519 public key from its seed, so this signs a
 * fixed, domain-separated message with the seed and verifies it against the
 * public key. A seed that does not correspond cannot produce a verifying
 * signature without forging one. Both keys are non-extractable.
 */
export async function signingSeedMatchesPublic(
	seed: Uint8Array,
	publicKey: Uint8Array,
): Promise<boolean> {
	const signingKey = await importSigningKey(seed);
	const signature = await globalThis.crypto.subtle.sign(
		{ name: 'Ed25519' },
		signingKey,
		KEY_CHECK_MESSAGE,
	);
	const verifyKey = await importVerifyKey(publicKey);
	return globalThis.crypto.subtle.verify(
		{ name: 'Ed25519' },
		verifyKey,
		signature,
		KEY_CHECK_MESSAGE,
	);
}
