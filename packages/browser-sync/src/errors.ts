/**
 * Public error codes and the structured error type for the browser sync client.
 *
 * Codes are stable, machine-readable, and safe to surface across an adapter
 * boundary. No message produced here includes secret material; callers must not
 * attach secrets to `cause` either.
 *
 * Zeroization is not guaranteed in JavaScript. This package clears references it
 * owns but does not claim reliable memory erasure.
 */

/** Stable public browser sync client error codes. */
export const BrowserSyncErrorCode = {
	/** A server response was missing fields or not valid JSON. */
	InvalidResponse: 'browser_sync_invalid_response',
	/** A request failed with a non-auth status or a transport error. */
	RequestFailed: 'browser_sync_request_failed',
	/** The request was rejected as unauthorized or forbidden. */
	Unauthorized: 'browser_sync_unauthorized',
	/** Device key material or a device identifier was malformed. */
	InvalidIdentity: 'browser_sync_invalid_identity',
	/** A wrapped bundle used an unknown version or key-derivation function. */
	UnsupportedBundleVersion: 'browser_sync_unsupported_bundle_version',
	/** A wrapped bundle was structurally invalid or used weak parameters. */
	InvalidBundle: 'browser_sync_invalid_bundle',
	/** A bundle could not be opened with the supplied passphrase. */
	PassphraseRejected: 'browser_sync_passphrase_rejected',
	/** A delivered envelope was signed by a device the caller did not pin. */
	UnpinnedSigner: 'browser_sync_unpinned_signer',
	/** A delivered envelope was addressed to a different recipient device. */
	RecipientMismatch: 'browser_sync_recipient_mismatch',
	/** A delivered envelope was malformed, failed verification, or failed to unwrap. */
	InvalidEnvelope: 'browser_sync_invalid_envelope',
	/** A delivered envelope used a construction this client does not support. */
	UnsupportedEnvelope: 'unsupported_envelope',
	/** An operation envelope was malformed, missed a field, or carried bad base64. */
	InvalidOperation: 'browser_sync_invalid_operation',
	/** An operation used a protocol version this client does not support. */
	UnsupportedOperationVersion: 'browser_sync_unsupported_operation_version',
	/** An operation signature did not verify against the trusted signing key. */
	InvalidOperationSignature: 'browser_sync_invalid_operation_signature',
	/** An operation failed AES-256-GCM authentication and could not be opened. */
	OperationDecryptFailed: 'browser_sync_operation_decrypt_failed',
} as const;

/** Union of the stable browser sync client error codes. */
export type BrowserSyncErrorCode =
	(typeof BrowserSyncErrorCode)[keyof typeof BrowserSyncErrorCode];

/** Options accepted by {@link BrowserSyncError}. */
export interface BrowserSyncErrorOptions {
	/** HTTP status code associated with the failure, when one exists. */
	status?: number;
	/** Underlying error. Must never carry secret material. */
	cause?: unknown;
}

/** Structured failure for browser sync client operations. */
export class BrowserSyncError extends Error {
	readonly code: BrowserSyncErrorCode;
	readonly status: number | undefined;
	readonly cause: unknown;

	constructor(
		code: BrowserSyncErrorCode,
		message?: string,
		options: BrowserSyncErrorOptions = {},
	) {
		super(message ?? code);
		this.name = 'BrowserSyncError';
		this.code = code;
		this.status = options.status;
		this.cause = options.cause;
	}
}
