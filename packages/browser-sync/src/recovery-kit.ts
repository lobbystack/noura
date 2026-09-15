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

import { isBrowserSyncBindingRecord } from './binding';
import type { BrowserSyncBindingRecord } from './binding';
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
	randomBytes,
} from './crypto';
import type { Bytes } from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import { isWrappedKeyBundle } from './identity';
import type { WrappedKeyBundle } from './identity';

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
