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
 * Managed sync remains end-to-end encrypted. This package never sends workspace
 * plaintext, never logs secrets, and never claims reliable memory zeroization.
 * Local replica reconciliation, outbox/conflict handling, revocation lock state,
 * recovery kits, and UI wiring are not implemented here; see
 * `docs/architecture/browser-sync.md`.
 */

export * from './errors';
export * from './crypto';
export * from './http';
export * from './identity';
export * from './enrollment';
export * from './keys';
export * from './operations';

export {
	decodeRecipient,
	deviceFingerprint,
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
