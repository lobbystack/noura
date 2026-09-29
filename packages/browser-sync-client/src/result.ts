/** Typed result helpers shared by the controller. */

import type { BrowserSyncFailureCode, BrowserSyncResult } from './types';

export function messageOf(error: unknown): string {
	if (error instanceof Error && error.message) return error.message;
	if (
		error &&
		typeof error === 'object' &&
		'message' in error &&
		typeof (error as { message?: unknown }).message === 'string'
	) {
		return String((error as { message: string }).message);
	}
	return 'Sync failed.';
}

export function ok<T>(value: T): BrowserSyncResult<T> {
	return { ok: true, value };
}

export function failure<T = undefined>(
	code: BrowserSyncFailureCode,
	message: string,
): BrowserSyncResult<T> {
	return { ok: false, code, message };
}
