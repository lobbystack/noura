/**
 * Parsing helpers for native `noura.sync.recovery` kit files.
 *
 * These validate the file wrapper and extract public fields. The signed
 * recovery object itself is verified by {@link importNativeRecoveryKit}.
 */

import { decodeBase64, isIdentifier } from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import {
	NATIVE_RECOVERY_DOMAIN,
	type NativeRecoveryEnvelope,
	type NativeRecoveryObject,
	type RecoverNativeKeysToBrowserBindingInput,
} from './recovery-kit';

/** Public fields extracted from a native `noura.sync.recovery` kit file. */
export interface ParsedNativeRecoveryKit {
	/** Signed public recovery object; the library validates its internals. */
	recovery: NativeRecoveryObject;
	/** Embedded age recovery identity when the kit records one, else `null`. */
	recoveryIdentity: string | null;
	/** Base64 Ed25519 recovery signer the recovery object self-describes. */
	recoverySignerPublic: string | null;
}

/**
 * The Ed25519 recovery signer a native recovery object self-describes.
 *
 * The recovery object's own (signed) configuration pins the public key of the
 * device that authored it. Returning it here lets the importer verify the
 * recovery signature, but only when the caller has not pinned a different key.
 */
export function nativeRecoverySignerPublic(
	recovery: NativeRecoveryObject,
): string | null {
	const config = recovery.config as unknown;
	if (!config || typeof config !== 'object' || Array.isArray(config))
		return null;
	const record = config as Record<string, unknown>;
	const deviceId = record.deviceId;
	const trusted = record.trustedDevices;
	if (typeof deviceId !== 'string') return null;
	if (!trusted || typeof trusted !== 'object' || Array.isArray(trusted))
		return null;
	const pin = (trusted as Record<string, unknown>)[deviceId];
	return typeof pin === 'string' ? pin : null;
}

/**
 * Parse a native `noura.sync.recovery` kit file.
 *
 * This validates the file wrapper and extracts the embedded recovery identity
 * and self-described recovery signer. The signed recovery object's own fields
 * and signatures are validated by `importNativeRecoveryKit`; a file that is not
 * a native recovery kit, including a browser `noura.browser-recovery-kit`, is
 * rejected rather than guessed.
 */
export function parseNativeRecoveryKit(file: unknown): ParsedNativeRecoveryKit {
	if (!file || typeof file !== 'object' || Array.isArray(file)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The native recovery kit was not an object.',
		);
	}
	const kit = file as Record<string, unknown>;
	const format = kit.format ?? kit.domain;
	if (format !== NATIVE_RECOVERY_DOMAIN) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The file was not a native noura recovery kit.',
		);
	}
	const recovery = kit.recovery;
	if (!recovery || typeof recovery !== 'object' || Array.isArray(recovery)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The native recovery kit was missing its recovery object.',
		);
	}
	const rawIdentity = kit.recovery_identity;
	if (
		rawIdentity !== undefined &&
		(typeof rawIdentity !== 'string' || rawIdentity.length === 0)
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidBundle,
			'The native recovery kit had a malformed recovery identity.',
		);
	}
	return {
		recovery: recovery as NativeRecoveryObject,
		recoveryIdentity: typeof rawIdentity === 'string' ? rawIdentity : null,
		recoverySignerPublic: nativeRecoverySignerPublic(
			recovery as NativeRecoveryObject,
		),
	};
}

/**
 * Extract the recovery identity embedded in a parsed native recovery kit, or
 * `null` when the kit does not record one. The caller must then require the user
 * to paste it. A malformed or non-native file throws instead of being ignored.
 */
export function extractEmbeddedRecoveryIdentity(file: unknown): string | null {
	return parseNativeRecoveryKit(file).recoveryIdentity;
}

/** Resolve the recovery signer, rejecting an inconsistent self-description. */
export function resolveNativeRecoverySigner(
	parsed: ParsedNativeRecoveryKit,
	pinned: string | undefined,
): { ok: true; value: Uint8Array } | { ok: false; message: string } {
	const selfDescribed = parsed.recoverySignerPublic;
	if (
		pinned !== undefined &&
		selfDescribed !== null &&
		pinned !== selfDescribed
	) {
		return {
			ok: false,
			message:
				"This kit's signing key doesn't match the one you gave, so noura won't trust either.",
		};
	}
	const chosen = pinned ?? selfDescribed;
	if (chosen === undefined || chosen === null) {
		return {
			ok: false,
			message: 'This kit is missing its signing key.',
		};
	}
	try {
		return { ok: true, value: decodeBase64(chosen, 32) };
	} catch {
		return {
			ok: false,
			message: "This kit's signing key is damaged.",
		};
	}
}

/**
 * Build the binding's per-object metadata from a native recovery object.
 *
 * A native kit does not carry the browser replica's canonical paths, so each
 * recovered object is anchored to its native object id. Object ids are
 * validated as identifiers (never canonical file paths) and the persisted
 * binding marks every recovered object `unmapped`, so a crafted kit cannot make
 * a local file or attachment be sealed under a recovered key. The binding holds
 * the re-wrapped key until the replica is reconciled.
 */
export function nativeRecoveryObjects(recovery: NativeRecoveryObject): {
	objectId: string;
	objects: RecoverNativeKeysToBrowserBindingInput['objects'];
} | null {
	const envelopes = Array.isArray(recovery.envelopes) ? recovery.envelopes : [];
	const highest = new Map<string, number>();
	for (const raw of envelopes) {
		const envelope = raw as NativeRecoveryEnvelope;
		if (
			!envelope ||
			!isIdentifier(envelope.objectId) ||
			!Number.isSafeInteger(envelope.epoch) ||
			envelope.epoch < 1
		)
			continue;
		const current = highest.get(envelope.objectId) ?? 0;
		if (envelope.epoch > current)
			highest.set(envelope.objectId, envelope.epoch);
	}
	const objects: RecoverNativeKeysToBrowserBindingInput['objects'] = {};
	let primary: string | null = null;
	for (const [objectId, epoch] of highest) {
		if (primary === null) primary = objectId;
		objects[objectId] = { path: objectId, epoch, policyRevision: '1' };
	}
	return primary === null ? null : { objectId: primary, objects };
}
