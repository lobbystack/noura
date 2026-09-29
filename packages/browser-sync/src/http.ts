/**
 * Minimal HTTP helpers shared by the browser sync client.
 *
 * The transport is an injected `fetch` so tests can avoid network access and the
 * host can supply an origin-bound or instrumented implementation. All requests
 * use `credentials: 'include'`.
 *
 * The sync server is untrusted: a response body is read incrementally and
 * aborted once it exceeds a caller-supplied bound, so a malicious or compromised
 * server cannot force an unbounded allocation.
 */

import { BrowserSyncError, BrowserSyncErrorCode } from './errors';

/** The subset of `fetch` this package requires. */
export type FetchLike = (
	input: RequestInfo | URL,
	init?: RequestInit,
) => Promise<Response>;

/**
 * Default ceiling for a JSON response body. Large enough for an access-state
 * page (up to 10,000 envelopes) but far below what would exhaust a tab. The
 * operation-pull path, whose page can legitimately reach ~100 MiB, passes a
 * larger explicit bound.
 */
export const MAX_JSON_RESPONSE_BYTES = 64 * 1024 * 1024;

function responseTooLarge(): BrowserSyncError {
	return new BrowserSyncError(
		BrowserSyncErrorCode.InvalidResponse,
		'response body exceeded the accepted size',
	);
}

/**
 * Read a response body into memory without exceeding `maxBytes`.
 *
 * When the platform exposes a streaming body the bytes are consumed chunk by
 * chunk and the read is cancelled as soon as the bound is crossed. A
 * `Content-Length` header, when present and numeric, is checked first. The
 * fallback path still verifies the final length.
 */
export async function readBoundedBytes(
	response: Response,
	maxBytes: number,
): Promise<Uint8Array> {
	const declared = response.headers.get('content-length');
	if (declared !== null && /^[0-9]+$/.test(declared)) {
		if (Number(declared) > maxBytes) throw responseTooLarge();
	}
	const body = response.body;
	if (!body) {
		const bytes = new Uint8Array(await response.arrayBuffer());
		if (bytes.length > maxBytes) throw responseTooLarge();
		return bytes;
	}
	const reader = body.getReader();
	const chunks: Uint8Array[] = [];
	let total = 0;
	try {
		for (;;) {
			const { done, value } = await reader.read();
			if (done) break;
			if (!value || value.length === 0) continue;
			total += value.length;
			if (total > maxBytes) {
				await reader.cancel().catch(() => {});
				throw responseTooLarge();
			}
			chunks.push(value);
		}
	} finally {
		reader.releaseLock();
	}
	const output = new Uint8Array(total);
	let offset = 0;
	for (const chunk of chunks) {
		output.set(chunk, offset);
		offset += chunk.length;
	}
	return output;
}

/**
 * Read a bounded JSON response body, mapping size and parse failures to a
 * structured error.
 */
export async function readJson(
	response: Response,
	maxBytes: number = MAX_JSON_RESPONSE_BYTES,
): Promise<unknown> {
	try {
		const bytes = await readBoundedBytes(response, maxBytes);
		return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
	} catch (cause) {
		if (cause instanceof BrowserSyncError) throw cause;
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response was not valid JSON',
			{ cause },
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

/**
 * Only same-origin requests are allowed. The hosted app is served by the sync
 * server, so a relative URL and the page origin address the same service.
 *
 * Redirects are refused (`redirect: 'error'`): checking only the initial URL
 * would let a malicious sync server answer with a 30x to an arbitrary origin.
 */
export function createSameOriginFetch(
	base: string = globalThis.location?.origin ?? '',
	fetchImpl: FetchLike = globalThis.fetch,
): FetchLike {
	return (input, init) => {
		if (base && !isSameOrigin(input, base)) {
			return Promise.reject(
				new BrowserSyncError(
					BrowserSyncErrorCode.RequestFailed,
					'Browser sync only uses same-origin requests',
				),
			);
		}
		return fetchImpl(input, { ...init, redirect: 'error' });
	};
}

function isSameOrigin(input: RequestInfo | URL, base: string): boolean {
	try {
		const resolved =
			typeof input === 'string'
				? new URL(input, base)
				: input instanceof URL
					? input
					: new URL(input.url, base);
		return resolved.origin === new URL(base).origin;
	} catch {
		return false;
	}
}
