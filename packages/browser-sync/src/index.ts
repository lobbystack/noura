/**
 * @noura/browser-sync — browser device custody, enrollment, and key delivery.
 *
 * This package implements the client foundation for browser workspace
 * synchronization:
 *
 * - Device identity generation and passphrase-wrapped at-rest custody
 *   ({@link createDeviceIdentity}, {@link unlockDeviceIdentity},
 *   {@link sealBundle}, {@link openBundle}, {@link KeyStore}).
 * - Device challenge and enrollment against the sync service
 *   ({@link requestDeviceChallenge}, {@link enrollBrowserDevice}).
 * - Signed object-key delivery with caller-pinned signers
 *   ({@link receiveKeys}).
 * - Encrypted operation sealing and opening plus the push/pull HTTP transport
 *   ({@link sealOperation}, {@link openOperation}, {@link BrowserSyncTransport}).
 *
 * - A user-held, passphrase-encrypted recovery kit that restores a device's
 *   wrapped bundle and binding on another browser ({@link exportRecoveryKit},
 *   {@link importRecoveryKit}).
 *
 * Managed sync remains end-to-end encrypted. This package never sends workspace
 * plaintext, never logs secrets, and never claims reliable memory zeroization.
 * Local replica reconciliation, outbox/conflict handling, revocation lock state,
 * and UI wiring are not implemented here; see
 * `docs/architecture/browser-sync.md`.
 */

export * from './errors';
export * from './crypto';
export * from './http';
export * from './identity';
export * from './binding';
export * from './enrollment';
export * from './keys';
export * from './operations';
export * from './file-change';
export * from './codec';
export * from './attachments';
export * from './access-policy';
export * from './recovery-kit';

export {
	AGE_DEVICE_FINGERPRINT_DOMAIN,
	DEVICE_FINGERPRINT_DOMAIN,
	decodeRecipient,
	deviceFingerprint,
	deviceFingerprintAge,
	deviceFingerprintForCard,
	encodeRecipient,
	enrollmentProof,
	KeyEnvelopeError,
	KeyEnvelopeErrorCode,
	unwrapKey,
	verifyEnvelope,
	wrapKey,
} from '@noura/sync-key-envelope';
export type {
	WebEnrollmentProofInputs,
	WebKeyEnvelope,
	WebKeyWrapInputs,
} from '@noura/sync-key-envelope';
