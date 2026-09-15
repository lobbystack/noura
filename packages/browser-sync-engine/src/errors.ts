/**
 * Public error codes and the structured error type for the browser sync engine.
 *
 * Codes are stable, machine-readable, and safe to surface across an adapter
 * boundary. No message produced here includes workspace plaintext or key
 * material; callers must not attach secrets to `cause` either.
 *
 * JavaScript cannot guarantee memory zeroization. This package clears the
 * references it owns but does not claim reliable memory erasure.
 */

/** Stable public browser sync engine error codes. */
export const BrowserSyncEngineErrorCode = {
	/** A cursor was not a canonical nonnegative decimal string. */
	InvalidCursor: 'browser_sync_engine_invalid_cursor',
	/** An operation could not be opened or had an unexpected shape. */
	InvalidOperation: 'browser_sync_engine_invalid_operation',
	/** A remote push or pull response had an unexpected shape. */
	InvalidResponse: 'browser_sync_engine_invalid_response',
	/** Durable sync state was malformed. */
	InvalidState: 'browser_sync_engine_invalid_state',
	/** A storage adapter was asked to perform a mutation it cannot express. */
	InvalidStorageCall: 'browser_sync_engine_invalid_storage_call',
	/** No recorded conflict matched the operation id being resolved. */
	ConflictNotFound: 'browser_sync_engine_conflict_not_found',
	/** A conflict resolution failed; no success is reported and the conflict was kept. */
	ResolveFailed: 'browser_sync_engine_resolve_failed',
	/** The remote rejected or failed a push; the outbox was left intact. */
	PushFailed: 'browser_sync_engine_push_failed',
	/** A pull failed; the durable cursor was not advanced. */
	PullFailed: 'browser_sync_engine_pull_failed',
	/** The device or workspace authorization was revoked. */
	Revoked: 'browser_sync_engine_revoked',
	/** The engine is locked and refuses further reconciliation. */
	Locked: 'browser_sync_engine_locked',
} as const;

/** Union of the stable browser sync engine error codes. */
export type BrowserSyncEngineErrorCode =
	(typeof BrowserSyncEngineErrorCode)[keyof typeof BrowserSyncEngineErrorCode];

/** Error code an injected codec or remote uses to signal revocation. */
export const REVOKED_ERROR_CODE = 'revoked';

/**
 * True when an injected boundary signalled revocation. Boundaries may throw a
 * {@link BrowserSyncEngineError} with the `Revoked` code or any error whose
 * `code` is `"revoked"`; both mean the same thing to the engine.
 */
export function isRevokedError(value: unknown): boolean {
	if (typeof value !== 'object' || value === null) return false;
	const code = (value as { code?: unknown }).code;
	return (
		code === REVOKED_ERROR_CODE || code === BrowserSyncEngineErrorCode.Revoked
	);
}

/** Options accepted by {@link BrowserSyncEngineError}. */
export interface BrowserSyncEngineErrorOptions {
	/** Underlying error. Must never carry workspace plaintext or key material. */
	cause?: unknown;
}

/** Structured failure for browser sync engine operations. */
export class BrowserSyncEngineError extends Error {
	readonly code: BrowserSyncEngineErrorCode;
	readonly cause: unknown;

	constructor(
		code: BrowserSyncEngineErrorCode,
		message?: string,
		options: BrowserSyncEngineErrorOptions = {},
	) {
		super(message ?? code);
		this.name = 'BrowserSyncEngineError';
		this.code = code;
		this.cause = options.cause;
	}
}
