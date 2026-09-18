/**
 * Minimal HTTP helpers shared by the browser sync client.
 *
 * The transport is an injected `fetch` so tests can avoid network access and the
 * host can supply an origin-bound or instrumented implementation. All requests
 * use `credentials: 'include'`.
 */

import { BrowserSyncError, BrowserSyncErrorCode } from './errors';

/** The subset of `fetch` this package requires. */
export type FetchLike = (
	input: RequestInfo | URL,
	init?: RequestInit,
) => Promise<Response>;

/** Read a JSON response body, mapping parse failures to a structured error. */
export async function readJson(response: Response): Promise<unknown> {
	try {
		return await response.json();
	} catch {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response was not valid JSON',
		);
	}
}

/** Throw a structured error unless the response is successful. */
export function ensureResponseOk(response: Response): void {
	if (response.ok) return;
	if (response.status === 401 || response.status === 403) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.Unauthorized,
			'request was not authorized',
			{ status: response.status },
		);
	}
	throw new BrowserSyncError(
		BrowserSyncErrorCode.RequestFailed,
		`request failed with status ${response.status}`,
		{ status: response.status },
	);
}
