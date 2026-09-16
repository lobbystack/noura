/**
 * Browser device identity generation and at-rest custody.
 *
 * A device identity is an Ed25519 signing key plus an X25519 recipient key. The
 * unwrapped secret material exists only in memory while the device is unlocked.
 * At rest it is stored only as a `WrappedKeyBundle`: the signing seed, the
 * X25519 secret, and the device bearer token encrypted with AES-256-GCM under a
 * PBKDF2-SHA256 key-encryption key derived from the user's passphrase.
 *
 * `sealBundle` performs the wrapping and `openBundle` performs the unwrapping.
 * `createDeviceIdentity` generates fresh key material and returns the wrapped
 * bundle; `unlockDeviceIdentity` returns the in-memory identity. A `KeyStore`
 * abstraction lets the host persist bundles in IndexedDB, OPFS, or another
 * origin store; an in-memory implementation is provided for tests.
 *
 * No unwrapped secret is written by this module. Never persist the object
 * returned by `unlockDeviceIdentity`.
 *
 * Zeroization is not guaranteed in JavaScript. This module clears references it
 * owns but does not claim reliable memory erasure.
 */

import { encodeRecipient } from '@noura/sync-key-envelope';
import {
	BUNDLE_DOMAIN,
	BUNDLE_KDF,
	BUNDLE_VERSION,
	PBKDF2_MIN_ITERATIONS,
	NONCE_LENGTH,
	SALT_LENGTH,
	SECRET_LENGTH,
	aesGcmDecrypt,
	aesGcmEncrypt,
	bytesEqual,
	canonicalBytes,
	decodeBase64,
	decodeUtf8,
	deriveKek,
	encodeBase64,
	encodeUtf8,
	fixedBytes,
	generateDeviceKeys,
	isIdentifier,
	randomBytes,
	randomIdentifier,
	type Bytes,
} from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';

const MIN_CIPHERTEXT_LENGTH = 16;

/**
 * An opaque, passphrase-wrapped device key bundle. Every field except the public
 * keys and the KDF parameters is ciphertext or non-secret metadata.
 */
export interface WrappedKeyBundle {
	/** Bundle format version. */
	version: 1;
	/** Stable identifier used as the {@link KeyStore} key. */
	id: string;
	/** Device identifier bound into the enrollment proof. */
	deviceId: string;
	/** Key-derivation function that produced the KEK. */
	kdf: 'pbkdf2-sha256';
	/** PBKDF2 iteration count. */
	iterations: number;
	/** Base64 PBKDF2 salt (32 bytes). */
	salt: string;
	/** Base64 AES-256-GCM nonce (12 bytes). */
	nonce: string;
	/** Base64 AES-256-GCM ciphertext and tag over the secret payload. */
	ciphertext: string;
	/** Base64 Ed25519 signing public key (32 bytes). */
	signingPublic: string;
	/** Base64 X25519 recipient public key (32 bytes). */
	recipientPublic: string;
}

/** Wrapped-bundle metadata with the ciphertext omitted, used as GCM AAD. */
type BundleMetadata = Omit<WrappedKeyBundle, 'ciphertext'>;

/** The decrypted payload held inside a {@link WrappedKeyBundle}. */
interface BundlePayload {
	signingSeed: string;
	x25519Secret: string;
	token: string;
}

/** In-memory device identity. Never persist this object. */
export interface DeviceIdentity {
	/** Device identifier bound into the enrollment proof. */
	deviceId: string;
	/** Ed25519 signing public key (32 bytes). */
	signingPublic: Bytes;
	/** X25519 recipient public key (32 bytes). */
	x25519Public: Bytes;
	/** Browser device recipient string (`x25519:` plus base64). */
	recipient: string;
	/** Ed25519 signing seed (32 bytes). In-memory only. */
	signingSeed: Bytes;
	/** X25519 recipient secret (32 bytes). In-memory only. */
	x25519Secret: Bytes;
	/** Device bearer token. Empty until the device is enrolled. */
	token: string;
}

/** Caller-supplied raw inputs for {@link sealBundle}. */
export interface SealBundleInput {
	/** User passphrase used to derive the KEK. */
	passphrase: string;
	/** Device identifier to bind. */
	deviceId: string;
	/** Ed25519 signing seed (32 bytes). */
	signingSeed: Uint8Array;
	/** X25519 recipient secret (32 bytes). */
	x25519Secret: Uint8Array;
	/** Ed25519 signing public key (32 bytes). */
	signingPublic: Uint8Array;
	/** X25519 recipient public key (32 bytes). */
	x25519Public: Uint8Array;
	/** Device bearer token. Defaults to empty. */
	token?: string;
	/** Bundle identifier. Defaults to a fresh random identifier. */
	bundleId?: string;
	/** PBKDF2 iteration count. Defaults to {@link PBKDF2_MIN_ITERATIONS}. */
	iterations?: number;
}

/** Options for {@link createDeviceIdentity}. */
export interface CreateDeviceIdentityOptions {
	/** User passphrase used to derive the KEK. */
	passphrase: string;
	/** Optional store to persist the wrapped bundle in. */
	keyStore?: KeyStore;
	/** Bundle identifier. Defaults to a fresh random identifier. */
	bundleId?: string;
	/** Device identifier. Defaults to a fresh random identifier. */
	deviceId?: string;
	/** Device bearer token. Defaults to empty. */
	token?: string;
	/** PBKDF2 iteration count. Defaults to {@link PBKDF2_MIN_ITERATIONS}. */
	iterations?: number;
}

/**
 * Injectable persistence for wrapped bundles. The host supplies an IndexedDB or
 * OPFS implementation; only wrapped ciphertext is ever passed in.
 */
export interface KeyStore {
	/** Read a wrapped bundle, or `undefined` when it does not exist. */
	read(id: string): Promise<WrappedKeyBundle | undefined>;
	/** Persist a wrapped bundle under `id`. */
	write(id: string, bundle: WrappedKeyBundle): Promise<void>;
	/** Delete a wrapped bundle. Missing entries are ignored. */
	delete(id: string): Promise<void>;
}

/** Create an in-memory {@link KeyStore} for tests and ephemeral sessions. */
export function createMemoryKeyStore(): KeyStore {
	const bundles = new Map<string, WrappedKeyBundle>();
	return {
		async read(id) {
			return bundles.get(id);
		},
		async write(id, bundle) {
			bundles.set(id, bundle);
		},
		async delete(id) {
			bundles.delete(id);
		},
	};
}

function bundleAad(metadata: BundleMetadata): Bytes {
	return canonicalBytes([
		BUNDLE_DOMAIN,
		metadata.version,
		metadata.id,
		metadata.deviceId,
		metadata.kdf,
		metadata.iterations,
		metadata.salt,
		metadata.nonce,
		metadata.signingPublic,
		metadata.recipientPublic,
	]);
}

function validateBundleMetadata(
	value: Record<string, unknown>,
): asserts value is Record<string, unknown> & BundleMetadata {
	if (value.version !== BUNDLE_VERSION) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UnsupportedBundleVersion,
			'unsupported bundle version',
		);
	}
	if (value.kdf !== BUNDLE_KDF) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UnsupportedBundleVersion,
			'unsupported bundle key-derivation function',
		);
	}
	if (
		typeof value.iterations !== 'number' ||
		!Number.isInteger(value.iterations) ||
		value.iterations < PBKDF2_MIN_ITERATIONS
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'bundle iteration count is below the minimum',
		);
	}
	for (const field of [
		'id',
		'deviceId',
		'salt',
		'nonce',
		'ciphertext',
		'signingPublic',
		'recipientPublic',
	]) {
		if (typeof value[field] !== 'string') {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidBundle,
				'bundle was missing a required field',
			);
		}
	}
	if (!isIdentifier(value.id) || !isIdentifier(value.deviceId)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'bundle identifiers were malformed',
		);
	}
}

/**
 * True when `value` is a structurally valid wrapped bundle.
 *
 * This reuses the same validation {@link openBundle} applies before decryption:
 * the format version, key-derivation function, iteration floor, required string
 * fields, and identifier shape. It performs no cryptography and never opens the
 * bundle, so it is safe for import paths that have no passphrase for the inner
 * bundle.
 */
export function isWrappedKeyBundle(value: unknown): value is WrappedKeyBundle {
	if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
	try {
		validateBundleMetadata(value as Record<string, unknown>);
		return true;
	} catch {
		return false;
	}
}

/**
 * Wrap raw device key material and a bearer token into an opaque bundle.
 *
 * The signing seed, X25519 secret, and token are serialized as canonical JSON,
 * encrypted with AES-256-GCM under a PBKDF2-SHA256 key-encryption key, and never
 * returned in the clear.
 */
export async function sealBundle(
	input: SealBundleInput,
): Promise<WrappedKeyBundle> {
	const iterations = input.iterations ?? PBKDF2_MIN_ITERATIONS;
	if (!Number.isInteger(iterations) || iterations < PBKDF2_MIN_ITERATIONS) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'iteration count is below the minimum',
		);
	}
	if (typeof input.passphrase !== 'string' || input.passphrase.length === 0) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'passphrase is required',
		);
	}
	if (!isIdentifier(input.deviceId)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidIdentity,
			'device identifier was malformed',
		);
	}
	const bundleId = input.bundleId ?? randomIdentifier();
	if (!isIdentifier(bundleId)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'bundle identifier was malformed',
		);
	}

	const signingSeed = fixedBytes(
		input.signingSeed,
		SECRET_LENGTH,
		'signing seed',
	);
	const x25519Secret = fixedBytes(
		input.x25519Secret,
		SECRET_LENGTH,
		'recipient secret',
	);
	const signingPublic = fixedBytes(
		input.signingPublic,
		SECRET_LENGTH,
		'signing public',
	);
	const x25519Public = fixedBytes(
		input.x25519Public,
		SECRET_LENGTH,
		'recipient public',
	);

	const salt = randomBytes(SALT_LENGTH);
	const nonce = randomBytes(NONCE_LENGTH);
	const metadata: BundleMetadata = {
		version: BUNDLE_VERSION,
		id: bundleId,
		deviceId: input.deviceId,
		kdf: BUNDLE_KDF,
		iterations,
		salt: encodeBase64(salt),
		nonce: encodeBase64(nonce),
		signingPublic: encodeBase64(signingPublic),
		recipientPublic: encodeBase64(x25519Public),
	};
	const payload: BundlePayload = {
		signingSeed: encodeBase64(signingSeed),
		x25519Secret: encodeBase64(x25519Secret),
		token: input.token ?? '',
	};
	const kek = await deriveKek(input.passphrase, salt, iterations);
	const ciphertext = await aesGcmEncrypt(
		kek,
		nonce,
		bundleAad(metadata),
		encodeUtf8(JSON.stringify(payload)),
	);
	return { ...metadata, ciphertext: encodeBase64(ciphertext) };
}

function parsePayload(plaintext: Bytes): BundlePayload {
	let parsed: unknown;
	try {
		parsed = JSON.parse(decodeUtf8(plaintext));
	} catch {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'bundle payload was not valid JSON',
		);
	}
	if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'bundle payload had an unexpected shape',
		);
	}
	const value = parsed as Record<string, unknown>;
	if (
		typeof value.signingSeed !== 'string' ||
		typeof value.x25519Secret !== 'string' ||
		typeof value.token !== 'string'
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'bundle payload was missing a required field',
		);
	}
	return {
		signingSeed: value.signingSeed,
		x25519Secret: value.x25519Secret,
		token: value.token,
	};
}

/**
 * Open a wrapped bundle with the user's passphrase, returning in-memory key
 * material. A wrong passphrase or tampered bundle fails authentication and is
 * reported as {@link BrowserSyncErrorCode.PassphraseRejected}.
 */
export async function openBundle(
	bundle: WrappedKeyBundle,
	passphrase: string,
): Promise<DeviceIdentity> {
	const value = bundle as unknown as Record<string, unknown>;
	validateBundleMetadata(value);

	const salt = decodeBase64(bundle.salt, SALT_LENGTH);
	const nonce = decodeBase64(bundle.nonce, NONCE_LENGTH);
	const ciphertext = decodeBase64(bundle.ciphertext);
	if (ciphertext.length < MIN_CIPHERTEXT_LENGTH) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'bundle ciphertext was too short',
		);
	}
	const signingPublic = decodeBase64(bundle.signingPublic, SECRET_LENGTH);
	const x25519Public = decodeBase64(bundle.recipientPublic, SECRET_LENGTH);

	let plaintext: Bytes;
	try {
		const kek = await deriveKek(passphrase, salt, bundle.iterations);
		plaintext = await aesGcmDecrypt(
			kek,
			nonce,
			bundleAad({
				version: bundle.version,
				id: bundle.id,
				deviceId: bundle.deviceId,
				kdf: bundle.kdf,
				iterations: bundle.iterations,
				salt: bundle.salt,
				nonce: bundle.nonce,
				signingPublic: bundle.signingPublic,
				recipientPublic: bundle.recipientPublic,
			}),
			ciphertext,
		);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.PassphraseRejected,
			'bundle could not be opened',
			{ cause },
		);
	}

	const payload = parsePayload(plaintext);
	const signingSeed = decodeBase64(payload.signingSeed, SECRET_LENGTH);
	const x25519Secret = decodeBase64(payload.x25519Secret, SECRET_LENGTH);
	if (bytesEqual(signingSeed, new Uint8Array(SECRET_LENGTH))) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'bundle signing seed was invalid',
		);
	}
	return {
		deviceId: bundle.deviceId,
		signingPublic,
		x25519Public,
		recipient: encodeRecipient(x25519Public),
		signingSeed,
		x25519Secret,
		token: payload.token,
	};
}

/**
 * Generate a fresh device identity and return only its wrapped bundle.
 *
 * When a {@link KeyStore} is supplied, the wrapped bundle is persisted under
 * `bundleId` (or the generated bundle id). The unwrapped secret is not returned.
 */
export async function createDeviceIdentity(
	options: CreateDeviceIdentityOptions,
): Promise<WrappedKeyBundle> {
	const deviceId = options.deviceId ?? randomIdentifier();
	const bundleId = options.bundleId ?? randomIdentifier();
	const keys = await generateDeviceKeys();
	const bundle = await sealBundle({
		passphrase: options.passphrase,
		deviceId,
		bundleId,
		signingSeed: keys.signingSeed,
		x25519Secret: keys.x25519Secret,
		signingPublic: keys.signingPublic,
		x25519Public: keys.x25519Public,
		token: options.token ?? '',
		...(options.iterations === undefined
			? {}
			: { iterations: options.iterations }),
	});
	if (options.keyStore) await options.keyStore.write(bundle.id, bundle);
	return bundle;
}

/** Unlock a wrapped bundle into an in-memory device identity. */
export function unlockDeviceIdentity(
	bundle: WrappedKeyBundle,
	passphrase: string,
): Promise<DeviceIdentity> {
	return openBundle(bundle, passphrase);
}

/**
 * Read a wrapped bundle from a {@link KeyStore} and unlock it.
 *
 * The bearer token returned by enrollment can be persisted by re-sealing the
 * unlocked identity with {@link sealIdentity}.
 */
export async function loadDeviceIdentity(
	keyStore: KeyStore,
	id: string,
	passphrase: string,
): Promise<DeviceIdentity> {
	const bundle = await keyStore.read(id);
	if (!bundle) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'bundle was not found in the key store',
		);
	}
	return openBundle(bundle, passphrase);
}

/**
 * Re-seal an unlocked identity, for example after recording an enrollment token.
 * Returns a new wrapped bundle; the caller decides whether to persist it.
 */
export function sealIdentity(
	identity: DeviceIdentity,
	passphrase: string,
	options: { bundleId?: string; iterations?: number } = {},
): Promise<WrappedKeyBundle> {
	return sealBundle({
		passphrase,
		deviceId: identity.deviceId,
		signingSeed: identity.signingSeed,
		x25519Secret: identity.x25519Secret,
		signingPublic: identity.signingPublic,
		x25519Public: identity.x25519Public,
		token: identity.token,
		...(options.bundleId === undefined ? {} : { bundleId: options.bundleId }),
		...(options.iterations === undefined
			? {}
			: { iterations: options.iterations }),
	});
}
