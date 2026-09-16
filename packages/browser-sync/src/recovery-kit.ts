/**
 * Browser recovery kit: an encrypted, user-held copy of a browser device's
 * wrapped key bundle and its durable sync binding.
 *
 * A recovery kit restores the **same** device identity, and its binding, on
 * another browser without a trusted device to approve or deliver keys. It is not
 * a new device. The kit carries the device's {@link WrappedKeyBundle} — the
 * passphrase-wrapped Ed25519 seed, X25519 secret, and bearer token — and its
 * {@link BrowserSyncBindingRecord}, which includes the remote workspace
 * identity, the self-wrapped object keys, and the pinned signer keys. It is
 * therefore **full credential material** and must be encrypted and held by the
 * user.
 *
 * This is a browser-specific format. It is not the signed native
 * `noura.sync.recovery` object; native interoperability remains a proposal (see
 * `docs/architecture/browser-sync.md`). A kit is portable across browsers
 * because it carries the wrapped bundle and binding directly rather than
 * depending on origin storage.
 *
 * At rest a kit is AES-256-GCM ciphertext under a PBKDF2-SHA256 key-encryption
 * key derived from a user passphrase. Unknown format, version, or key-derivation
 * function values are rejected, never guessed. This module never logs secret
 * material and does not claim reliable memory zeroization.
 */

import { wrapKey } from '@noura/sync-key-envelope';
import { decryptAgeCiphertext, decodeAgeSecretIdentity } from './attachments';
import { isBrowserSyncBindingRecord } from './binding';
import type {
	BrowserSyncBoundKey,
	BrowserSyncBoundObject,
	BrowserSyncBindingRecord,
} from './binding';
import {
	NONCE_LENGTH,
	PBKDF2_MIN_ITERATIONS,
	SALT_LENGTH,
	SECRET_LENGTH,
	aesGcmDecrypt,
	aesGcmEncrypt,
	canonicalBytes,
	decodeBase64,
	decodeUtf8,
	deriveKek,
	encodeBase64,
	encodeUtf8,
	fixedBytes,
	importVerifyKey,
	isIdentifier,
	randomBytes,
} from './crypto';
import type { Bytes } from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import { isWrappedKeyBundle } from './identity';
import type { DeviceIdentity, WrappedKeyBundle } from './identity';

/** Exact recovery-kit format discriminator. */
export const RECOVERY_KIT_FORMAT = 'noura.browser-recovery-kit';

/** Supported recovery-kit format version. */
export const RECOVERY_KIT_VERSION = 1;

/** Supported key-derivation function identifier. */
export const RECOVERY_KIT_KDF = 'pbkdf2-sha256';

/** Domain string bound as AES-GCM additional authenticated data. */
const RECOVERY_KIT_AAD_DOMAIN = 'noura.browser-recovery-kit.aad.v1';

/** Minimum decoded ciphertext length that still contains a GCM tag. */
const MIN_CIPHERTEXT_LENGTH = 16;

/** Maximum decoded ciphertext bytes accepted on import. */
export const MAX_RECOVERY_KIT_CIPHERTEXT_BYTES = 4 * 1024 * 1024;

/** Maximum base64 length accepted before decoding, derived from the byte cap. */
const MAX_RECOVERY_KIT_CIPHERTEXT_CHARS =
	Math.ceil((MAX_RECOVERY_KIT_CIPHERTEXT_BYTES * 4) / 3) + 8;

/** A user-held, passphrase-encrypted browser recovery kit. */
export interface RecoveryKitFile {
	format: 'noura.browser-recovery-kit';
	version: 1;
	createdAt: string;
	kdf: 'pbkdf2-sha256';
	iterations: number;
	salt: string;
	nonce: string;
	ciphertext: string;
}

/** Inputs for {@link exportRecoveryKit}. */
export interface ExportRecoveryKitInput {
	/** The device's wrapped key bundle. Full credential material. */
	bundle: WrappedKeyBundle;
	/** The device's durable browser sync binding record. */
	binding: BrowserSyncBindingRecord;
	/** User passphrase protecting the kit. */
	passphrase: string;
	/** Key-derivation function. Only `pbkdf2-sha256` is supported. */
	kdf?: 'pbkdf2-sha256';
	/** PBKDF2 iteration count. Defaults to the bundle's count. */
	iterations?: number;
}

/** Decrypted recovery-kit contents. */
export interface RecoveryKitPayload {
	bundle: WrappedKeyBundle;
	binding: BrowserSyncBindingRecord;
}

function recoveryKitAad(): Bytes {
	return canonicalBytes([
		RECOVERY_KIT_AAD_DOMAIN,
		RECOVERY_KIT_FORMAT,
		RECOVERY_KIT_VERSION,
	]);
}

/**
 * True when a wrapped bundle's base64 fields decode to their required lengths.
 *
 * Complements the structural {@link isWrappedKeyBundle} check so a bundle with a
 * malformed salt, nonce, public key, or ciphertext is rejected before it is
 * placed in a kit or accepted from one.
 */
function hasValidBundleEncodings(bundle: WrappedKeyBundle): boolean {
	try {
		decodeBase64(bundle.salt, SALT_LENGTH);
		decodeBase64(bundle.nonce, NONCE_LENGTH);
		decodeBase64(bundle.signingPublic, SECRET_LENGTH);
		decodeBase64(bundle.recipientPublic, SECRET_LENGTH);
		decodeBase64(bundle.ciphertext);
		return true;
	} catch {
		return false;
	}
}

/**
 * Deterministic JSON with sorted object keys.
 *
 * The kit is encrypted and decrypted only by this module, so the ordering needs
 * to be stable rather than shared with a native format. Undefined object values
 * are dropped to match `JSON.stringify`.
 */
function canonicalJson(value: unknown): string {
	if (value === undefined) return 'null';
	if (Array.isArray(value)) {
		return `[${value.map((entry) => canonicalJson(entry)).join(',')}]`;
	}
	if (value && typeof value === 'object') {
		const record = value as Record<string, unknown>;
		const keys = Object.keys(record)
			.filter((key) => record[key] !== undefined)
			.sort();
		const members = keys.map(
			(key) => `${JSON.stringify(key)}:${canonicalJson(record[key])}`,
		);
		return `{${members.join(',')}}`;
	}
	return JSON.stringify(value) ?? 'null';
}

/**
 * Encrypt a device's wrapped bundle and binding into a user-held recovery kit.
 *
 * The kit contains full credential material and is only ever returned as
 * ciphertext. Unknown `kdf` values, an empty passphrase, an invalid bundle or
 * binding, and an iteration count below
 * {@link PBKDF2_MIN_ITERATIONS} are rejected.
 */
export async function exportRecoveryKit(
	input: ExportRecoveryKitInput,
): Promise<RecoveryKitFile> {
	const kdf = input.kdf ?? RECOVERY_KIT_KDF;
	if (kdf !== RECOVERY_KIT_KDF) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UnsupportedBundleVersion,
			'the recovery kit key-derivation function is not supported',
		);
	}
	if (typeof input.passphrase !== 'string' || input.passphrase.length === 0) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'a recovery kit passphrase is required',
		);
	}
	if (!isWrappedKeyBundle(input.bundle)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit bundle was not a valid wrapped key bundle',
		);
	}
	if (!hasValidBundleEncodings(input.bundle)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit bundle had malformed key material',
		);
	}
	if (!isBrowserSyncBindingRecord(input.binding)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit binding was not a valid binding record',
		);
	}
	const iterations = input.iterations ?? input.bundle.iterations;
	if (!Number.isInteger(iterations) || iterations < PBKDF2_MIN_ITERATIONS) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit iteration count is below the minimum',
		);
	}

	const salt = randomBytes(SALT_LENGTH);
	const nonce = randomBytes(NONCE_LENGTH);
	const plaintext = encodeUtf8(
		canonicalJson({ bundle: input.bundle, binding: input.binding }),
	);
	const kek = await deriveKek(input.passphrase, salt, iterations);
	const ciphertext = await aesGcmEncrypt(
		kek,
		nonce,
		recoveryKitAad(),
		plaintext,
	);
	return {
		format: RECOVERY_KIT_FORMAT,
		version: RECOVERY_KIT_VERSION,
		createdAt: new Date().toISOString(),
		kdf: RECOVERY_KIT_KDF,
		iterations,
		salt: encodeBase64(salt),
		nonce: encodeBase64(nonce),
		ciphertext: encodeBase64(ciphertext),
	};
}

/**
 * Decrypt and validate a recovery kit with its passphrase.
 *
 * Validates the format, version, key-derivation function, base64 fields, decoded
 * lengths, and the recovered bundle and binding shapes. A wrong passphrase or
 * tampered ciphertext fails AES-GCM authentication and is reported as
 * {@link BrowserSyncErrorCode.PassphraseRejected}. An unknown format or version
 * is rejected rather than guessed. Oversized input is rejected before decryption.
 */
export async function importRecoveryKit(
	file: unknown,
	passphrase: string,
): Promise<RecoveryKitPayload> {
	if (typeof passphrase !== 'string' || passphrase.length === 0) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'a recovery kit passphrase is required',
		);
	}
	if (!file || typeof file !== 'object' || Array.isArray(file)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit was not an object',
		);
	}
	const kit = file as Record<string, unknown>;
	if (kit.format !== RECOVERY_KIT_FORMAT) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the file was not a Noura browser recovery kit',
		);
	}
	if (kit.version !== RECOVERY_KIT_VERSION) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UnsupportedBundleVersion,
			'unsupported recovery kit version',
		);
	}
	if (kit.kdf !== RECOVERY_KIT_KDF) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UnsupportedBundleVersion,
			'unsupported recovery kit key-derivation function',
		);
	}
	if (
		typeof kit.createdAt !== 'string' ||
		Number.isNaN(Date.parse(kit.createdAt))
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit creation time was malformed',
		);
	}
	if (
		typeof kit.iterations !== 'number' ||
		!Number.isInteger(kit.iterations) ||
		kit.iterations < PBKDF2_MIN_ITERATIONS
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit iteration count is below the minimum',
		);
	}
	if (
		typeof kit.salt !== 'string' ||
		typeof kit.nonce !== 'string' ||
		typeof kit.ciphertext !== 'string'
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit was missing a required field',
		);
	}
	if (kit.ciphertext.length > MAX_RECOVERY_KIT_CIPHERTEXT_CHARS) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit was too large',
		);
	}

	const salt = decodeBase64(kit.salt, SALT_LENGTH);
	const nonce = decodeBase64(kit.nonce, NONCE_LENGTH);
	const ciphertext = decodeBase64(kit.ciphertext);
	if (ciphertext.length < MIN_CIPHERTEXT_LENGTH) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit ciphertext was too short',
		);
	}
	if (ciphertext.length > MAX_RECOVERY_KIT_CIPHERTEXT_BYTES) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit was too large',
		);
	}

	let plaintext: Bytes;
	try {
		const kek = await deriveKek(passphrase, salt, kit.iterations);
		plaintext = await aesGcmDecrypt(kek, nonce, recoveryKitAad(), ciphertext);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.PassphraseRejected,
			'the recovery kit could not be opened',
			{ cause },
		);
	}
	if (plaintext.length > MAX_RECOVERY_KIT_CIPHERTEXT_BYTES) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit payload was too large',
		);
	}

	let parsed: unknown;
	try {
		parsed = JSON.parse(decodeUtf8(plaintext));
	} catch {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit payload was not valid JSON',
		);
	}
	if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit payload had an unexpected shape',
		);
	}
	const payload = parsed as Record<string, unknown>;
	const bundle = payload.bundle;
	const binding = payload.binding;
	if (!isWrappedKeyBundle(bundle)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit bundle was invalid',
		);
	}
	if (!hasValidBundleEncodings(bundle)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit bundle had malformed key material',
		);
	}
	if (!isBrowserSyncBindingRecord(binding)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery kit binding was invalid',
		);
	}
	return { bundle, binding };
}

/** Exact native recovery-object domain string. */
export const NATIVE_RECOVERY_DOMAIN = 'noura.sync.recovery';

/** Supported native recovery-object version. */
export const NATIVE_RECOVERY_VERSION = 1;

/** Native recovery envelope cap, matching `RecoveryKit::reader`. */
export const MAX_NATIVE_RECOVERY_ENVELOPES = 1000;

/**
 * Native `noura.sync.recovery` workspace sync configuration snapshot.
 *
 * This mirrors `WorkspaceSyncConfig`'s camelCase wire shape. Maps are sorted by
 * key on the Rust side, so callers must treat them as unordered.
 */
export interface NativeRecoveryConfig {
	version: number;
	workspaceId: string;
	origin: string;
	deviceId: string;
	enabled: boolean;
	trustedDevices: Record<string, string>;
	approvedRecipients: Record<string, string>;
	approvedAccounts: Record<string, string>;
}

/** One native `age` object-key envelope (construction `age`). */
export interface NativeRecoveryEnvelope {
	workspaceId: string;
	objectId: string;
	epoch: number;
	deviceId: string;
	wrappedKey: string;
	signingDevice: string;
	signature: string;
}

/**
 * Public portion of a native `noura.sync.recovery` kit.
 *
 * A native kit file also embeds the recovery identity secret. This type carries
 * only the public fields; the caller passes the identity to
 * {@link importNativeRecoveryKit} separately so the browser never has to retain
 * or persist the secret alongside the signed object.
 */
export interface NativeRecoveryObject {
	version: number;
	config: NativeRecoveryConfig;
	envelopes: NativeRecoveryEnvelope[];
	signature: string;
}

/** Inputs for {@link importNativeRecoveryKit}. */
export interface ImportNativeRecoveryKitInput {
	/** Public recovery object read from a native kit. */
	recovery: NativeRecoveryObject;
	/** User-supplied native age recovery identity secret (`AGE-SECRET-KEY-...`). */
	recoveryIdentity: string;
	/** Caller-pinned Ed25519 public key (32 bytes) of the recovery signer. */
	trustedRecoverySigner: Uint8Array;
}

/** One recovered native object key, with its authenticated routing metadata. */
export interface RecoveredNativeObjectKey {
	objectId: string;
	epoch: number;
	key: Bytes;
	signingDevice: string;
}

/** Result of {@link importNativeRecoveryKit}. */
export interface NativeRecoveryImportResult {
	/** Convenience map keyed by object id; the highest epoch wins on repeat. */
	keys: Map<string, Bytes>;
	/** Every recovered key in kit order, retaining its epoch and signer. */
	entries: RecoveredNativeObjectKey[];
}

/** Inputs for {@link recoverNativeKeysToBrowserBinding}. */
export interface RecoverNativeKeysToBrowserBindingInput {
	/** Public recovery object read from a native kit. */
	recovery: NativeRecoveryObject;
	/** User-supplied native age recovery identity secret. */
	recoveryIdentity: string;
	/** Caller-pinned Ed25519 public key of the recovery signer. */
	trustedRecoverySigner: Uint8Array;
	/** Unlocked browser device the recovered keys are re-wrapped to. */
	device: DeviceIdentity;
	/** Stable id of the local browser workspace the binding belongs to. */
	localWorkspaceId: string;
	/** Access-policy revision the binding is created at. */
	revision: string;
	/** Primary sync object id; must be present in `objects`. */
	objectId: string;
	/** Local path/epoch metadata per recovered object id. */
	objects: Record<
		string,
		{
			path: string;
			localObjectId?: string;
			epoch: number;
			policyRevision: string;
		}
	>;
}

/** Result of {@link recoverNativeKeysToBrowserBinding}. */
export interface RecoverNativeKeysToBrowserBindingResult {
	/** Recovered plaintext object keys. The caller must not persist these. */
	keys: Map<string, Bytes>;
	/** Binding record whose object keys are self-wrapped `noura.sync.key.web`. */
	binding: BrowserSyncBindingRecord;
}

function isStringRecord(value: unknown): value is Record<string, string> {
	if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
	return Object.values(value).every((entry) => typeof entry === 'string');
}

function isNativeRecoveryConfig(value: unknown): value is NativeRecoveryConfig {
	if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
	const config = value as Record<string, unknown>;
	return (
		typeof config.version === 'number' &&
		config.version === NATIVE_RECOVERY_VERSION &&
		// Identifiers are bound into signed tuples and later used as routing and
		// binding keys; reject path-like or otherwise malformed values up front.
		isIdentifier(config.workspaceId) &&
		typeof config.origin === 'string' &&
		isIdentifier(config.deviceId) &&
		typeof config.enabled === 'boolean' &&
		isStringRecord(config.trustedDevices) &&
		isStringRecord(config.approvedRecipients) &&
		isStringRecord(config.approvedAccounts)
	);
}

function isNativeRecoveryEnvelope(
	value: unknown,
): value is NativeRecoveryEnvelope {
	if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
	const envelope = value as Record<string, unknown>;
	return (
		isIdentifier(envelope.workspaceId) &&
		isIdentifier(envelope.objectId) &&
		typeof envelope.epoch === 'number' &&
		Number.isSafeInteger(envelope.epoch) &&
		envelope.epoch > 0 &&
		isIdentifier(envelope.deviceId) &&
		typeof envelope.wrappedKey === 'string' &&
		isIdentifier(envelope.signingDevice) &&
		typeof envelope.signature === 'string'
	);
}

function sortedStringRecord(
	record: Record<string, string>,
): Record<string, string> {
	const sorted: Record<string, string> = {};
	for (const key of Object.keys(record).sort()) sorted[key] = record[key]!;
	return sorted;
}

/**
 * Rebuild the native `WorkspaceSyncConfig` in Rust's declaration order.
 *
 * `serde_json` serializes struct fields in declaration order and `BTreeMap`
 * entries in sorted key order. Rebuilding the object this way makes the signed
 * bytes independent of how the caller's parsed JSON happened to be ordered.
 */
function nativeConfigForSigning(
	config: NativeRecoveryConfig,
): Record<string, unknown> {
	return {
		version: config.version,
		workspaceId: config.workspaceId,
		origin: config.origin,
		deviceId: config.deviceId,
		enabled: config.enabled,
		trustedDevices: sortedStringRecord(config.trustedDevices),
		approvedRecipients: sortedStringRecord(config.approvedRecipients),
		approvedAccounts: sortedStringRecord(config.approvedAccounts),
	};
}

/**
 * Rebuild a native `age` `KeyEnvelope` in Rust's declaration order, omitting the
 * web-only discriminator and fields that are skipped for the age construction.
 */
function nativeEnvelopeForSigning(
	envelope: NativeRecoveryEnvelope,
): Record<string, unknown> {
	return {
		workspaceId: envelope.workspaceId,
		objectId: envelope.objectId,
		epoch: envelope.epoch,
		deviceId: envelope.deviceId,
		wrappedKey: envelope.wrappedKey,
		signingDevice: envelope.signingDevice,
		signature: envelope.signature,
	};
}

/** Canonical bytes covered by the native recovery signature. */
function nativeRecoverySigningBytes(
	recovery: NativeRecoveryObject,
	recoveryIdentity: string,
): Bytes {
	return encodeUtf8(
		JSON.stringify([
			NATIVE_RECOVERY_DOMAIN,
			recovery.version,
			nativeConfigForSigning(recovery.config),
			recoveryIdentity,
			recovery.envelopes.map(nativeEnvelopeForSigning),
		]),
	);
}

/** Canonical bytes covered by a native `age` key envelope's signature. */
function nativeEnvelopeSigningBytes(envelope: NativeRecoveryEnvelope): Bytes {
	return encodeUtf8(
		JSON.stringify([
			'noura.sync.key',
			1,
			envelope.workspaceId,
			envelope.objectId,
			envelope.epoch,
			envelope.signingDevice,
			envelope.deviceId,
			envelope.wrappedKey,
		]),
	);
}

async function verifyNativeSignature(
	publicKey: Uint8Array,
	signature: Bytes,
	message: Bytes,
): Promise<boolean> {
	const key = await importVerifyKey(publicKey);
	return globalThis.crypto.subtle.verify(
		{ name: 'Ed25519' },
		key,
		signature,
		message,
	);
}

/**
 * Verify and unwrap the native `age` object keys in a recovery object.
 *
 * The recovery signature is checked against the caller-pinned recovery signer
 * before any envelope is trusted; the recovery identity secret is part of the
 * signed tuple, so a wrong secret cannot verify. Each envelope is then verified
 * against the signer pinned in the (signed) recovery configuration and unwrapped
 * with the user-supplied identity. Envelope signers are authenticated by the
 * recovery signature, never by the server.
 *
 * The recovery identity and the returned plaintext keys exist only in memory;
 * callers must never persist them. Only `noura.sync.key.web` self-wrapped keys
 * may be stored, which {@link recoverNativeKeysToBrowserBinding} produces.
 */
export async function importNativeRecoveryKit(
	input: ImportNativeRecoveryKitInput,
): Promise<NativeRecoveryImportResult> {
	const recovery = input.recovery;
	if (!recovery || typeof recovery !== 'object') {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'the native recovery object was not an object',
		);
	}
	if (recovery.version !== NATIVE_RECOVERY_VERSION) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UnsupportedBundleVersion,
			'unsupported native recovery object version',
		);
	}
	if (!isNativeRecoveryConfig(recovery.config)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'the native recovery configuration was malformed',
		);
	}
	if (
		!Array.isArray(recovery.envelopes) ||
		recovery.envelopes.length > MAX_NATIVE_RECOVERY_ENVELOPES
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'the native recovery envelopes were malformed',
		);
	}
	if (
		typeof input.recoveryIdentity !== 'string' ||
		input.recoveryIdentity.length === 0
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'a native recovery identity is required',
		);
	}

	let trustedSigner: Bytes;
	try {
		trustedSigner = fixedBytes(
			input.trustedRecoverySigner,
			32,
			'recovery signer',
		);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'the pinned recovery signer was malformed',
			{ cause },
		);
	}

	let recoverySignature: Bytes;
	try {
		recoverySignature = decodeBase64(recovery.signature, 64);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidSignature,
			'the native recovery signature was malformed',
			{ cause },
		);
	}
	let recoveryVerified: boolean;
	try {
		recoveryVerified = await verifyNativeSignature(
			trustedSigner,
			recoverySignature,
			nativeRecoverySigningBytes(recovery, input.recoveryIdentity),
		);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidSignature,
			'the native recovery signature could not be checked',
			{ cause },
		);
	}
	if (!recoveryVerified) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidSignature,
			'the native recovery signature did not verify',
		);
	}

	let secret: Bytes;
	try {
		secret = decodeAgeSecretIdentity(input.recoveryIdentity);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidEnvelope,
			'the native recovery identity was malformed',
			{ cause },
		);
	}

	const keys = new Map<string, Bytes>();
	const highestEpoch = new Map<string, number>();
	const entries: RecoveredNativeObjectKey[] = [];
	const seen = new Set<string>();
	for (const raw of recovery.envelopes) {
		if (!isNativeRecoveryEnvelope(raw)) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidEnvelope,
				'a native recovery envelope was malformed',
			);
		}
		if (
			raw.workspaceId !== recovery.config.workspaceId ||
			raw.deviceId !== recovery.config.deviceId
		) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidEnvelope,
				'a native recovery envelope did not match its recovery context',
			);
		}
		const identity = `${raw.objectId}:${raw.epoch}`;
		if (seen.has(identity)) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidEnvelope,
				'a native recovery envelope was duplicated',
			);
		}
		seen.add(identity);

		const signerPin = recovery.config.trustedDevices[raw.signingDevice];
		if (!signerPin) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.UnpinnedSigner,
				'a native recovery envelope signer was not pinned',
			);
		}
		let signerBytes: Bytes;
		let envelopeSignature: Bytes;
		try {
			signerBytes = decodeBase64(signerPin, 32);
			envelopeSignature = decodeBase64(raw.signature, 64);
		} catch (cause) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidEnvelope,
				'a native recovery envelope signature was malformed',
				{ cause },
			);
		}
		let verified: boolean;
		try {
			verified = await verifyNativeSignature(
				signerBytes,
				envelopeSignature,
				nativeEnvelopeSigningBytes(raw),
			);
		} catch (cause) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidEnvelope,
				'a native recovery envelope signature could not be checked',
				{ cause },
			);
		}
		if (!verified) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidSignature,
				'a native recovery envelope signature did not verify',
			);
		}

		let key: Bytes;
		try {
			const wrapped = decodeBase64(raw.wrappedKey);
			const plaintext = await decryptAgeCiphertext(secret, wrapped);
			const parsed: unknown = JSON.parse(decodeUtf8(plaintext));
			if (!Array.isArray(parsed) || parsed.length !== 6) {
				throw new Error('unexpected wrapped key tuple');
			}
			const [domain, version, workspaceId, objectId, epoch, encoded] =
				parsed as unknown[];
			if (
				domain !== 'noura.sync.object-key' ||
				version !== 1 ||
				workspaceId !== raw.workspaceId ||
				objectId !== raw.objectId ||
				epoch !== raw.epoch ||
				typeof encoded !== 'string'
			) {
				throw new Error('wrapped key did not match its envelope');
			}
			key = decodeBase64(encoded, 32);
		} catch (cause) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidEnvelope,
				'a native recovery envelope could not be unwrapped',
				{ cause },
			);
		}

		entries.push({
			objectId: raw.objectId,
			epoch: raw.epoch,
			key,
			signingDevice: raw.signingDevice,
		});
		const current = highestEpoch.get(raw.objectId) ?? 0;
		if (raw.epoch >= current) {
			highestEpoch.set(raw.objectId, raw.epoch);
			keys.set(raw.objectId, key);
		}
	}

	return { keys, entries };
}

/** Convert a self-wrapped key envelope into the persisted binding shape. */
function toBoundKey(
	envelope: {
		wrapped_key: string;
		signature: string;
		recipient_public_key: string;
		ephemeral_public_key: string;
		salt: string;
		nonce: string;
	},
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

/**
 * Recover native object keys and re-wrap them to the unlocked browser device as
 * a durable binding record.
 *
 * This is the library-level interop path: after a native kit is verified and its
 * keys unwrapped, each requested object key is wrapped as a self-addressed
 * `noura.sync.key.web` envelope signed by the browser device, exactly as the
 * browser binding path does elsewhere. The returned record contains only
 * ciphertext and public pins, so the caller must persist the record and discard
 * the plaintext keys. The recovery identity is never stored.
 */
export async function recoverNativeKeysToBrowserBinding(
	input: RecoverNativeKeysToBrowserBindingInput,
): Promise<RecoverNativeKeysToBrowserBindingResult> {
	const recovered = await importNativeRecoveryKit({
		recovery: input.recovery,
		recoveryIdentity: input.recoveryIdentity,
		trustedRecoverySigner: input.trustedRecoverySigner,
	});
	if (
		!isIdentifier(input.localWorkspaceId) ||
		!isIdentifier(input.objectId) ||
		!Object.keys(input.objects).every((objectId) => isIdentifier(objectId))
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the recovery binding used a malformed workspace or object identifier',
		);
	}
	if (!(input.objectId in input.objects)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'the binding primary object was not described',
		);
	}
	const objects: Record<string, BrowserSyncBoundObject> = {};
	for (const [objectId, bound] of Object.entries(input.objects)) {
		const entry = recovered.entries.find(
			(candidate) =>
				candidate.objectId === objectId && candidate.epoch === bound.epoch,
		);
		if (!entry) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.MissingKey,
				'the native recovery object did not carry the requested key',
			);
		}
		const envelope = await wrapKey({
			workspace_id: input.recovery.config.workspaceId,
			object_id: objectId,
			epoch: bound.epoch,
			signing_device: input.device.deviceId,
			device_id: input.device.deviceId,
			signing_secret: encodeBase64(input.device.signingSeed),
			recipient_public: encodeBase64(input.device.x25519Public),
			object_key: encodeBase64(entry.key),
			ephemeral_secret: encodeBase64(randomBytes(32)),
			salt: encodeBase64(randomBytes(32)),
			nonce: encodeBase64(randomBytes(12)),
		});
		objects[objectId] = {
			path: bound.path,
			...(bound.localObjectId === undefined
				? {}
				: { localObjectId: bound.localObjectId }),
			epoch: bound.epoch,
			policyRevision: bound.policyRevision,
			// A native kit carries no verified canonical paths, so the recovered
			// object must not be treated as the owner of a local file.
			unmapped: true,
			key: toBoundKey(envelope, input.device.deviceId),
		};
	}
	return {
		keys: recovered.keys,
		binding: {
			version: 1,
			localWorkspaceId: input.localWorkspaceId,
			workspaceId: input.recovery.config.workspaceId,
			revision: input.revision,
			objectId: input.objectId,
			objects,
			pinnedSigners: {
				[input.device.deviceId]: encodeBase64(input.device.signingPublic),
			},
		},
	};
}
