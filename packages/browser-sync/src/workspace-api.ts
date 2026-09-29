/**
 * Remote workspace, object, and access-policy calls used when a browser
 * bootstraps or provisions encrypted sync. Requests carry the device bearer
 * token; callers supply a same-origin fetch.
 */

import type { AccessPolicy } from './access-policy';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import { ensureResponseOk, readJson, type FetchLike } from './http';

function authorizationHeaders(token: string): Record<string, string> {
	return {
		authorization: `Bearer ${token}`,
		'content-type': 'application/json',
		accept: 'application/json',
	};
}

/** Create the remote workspace that a browser-only first device owns. */
export async function createRemoteWorkspace(input: {
	origin: string;
	token: string;
	fetch: FetchLike;
	workspaceId: string;
}): Promise<void> {
	const response = await input.fetch(
		new URL('/v1/workspaces', input.origin).toString(),
		{
			method: 'POST',
			credentials: 'include',
			headers: authorizationHeaders(input.token),
			body: JSON.stringify({ id: input.workspaceId }),
		},
	);
	ensureResponseOk(response);
}

/** Create one remote sync object and return its key epoch. */
export async function createRemoteObject(input: {
	origin: string;
	token: string;
	fetch: FetchLike;
	workspaceId: string;
	objectId: string;
}): Promise<number> {
	const response = await input.fetch(
		new URL(
			`/v1/workspaces/${encodeURIComponent(input.workspaceId)}/objects`,
			input.origin,
		).toString(),
		{
			method: 'POST',
			credentials: 'include',
			headers: authorizationHeaders(input.token),
			body: JSON.stringify({ id: input.objectId }),
		},
	);
	ensureResponseOk(response);
	const data = (await readJson(response)) as { epoch?: unknown } | null;
	const epoch = data?.epoch;
	if (typeof epoch !== 'number' || !Number.isSafeInteger(epoch) || epoch < 1) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'The sync server returned an invalid object epoch',
		);
	}
	return epoch;
}

/** Upload the signed version-1 access policy for the new workspace. */
export async function putRemoteAccessPolicy(input: {
	origin: string;
	token: string;
	fetch: FetchLike;
	workspaceId: string;
	policy: AccessPolicy;
}): Promise<void> {
	const response = await input.fetch(
		new URL(
			`/v1/workspaces/${encodeURIComponent(input.workspaceId)}/access`,
			input.origin,
		).toString(),
		{
			method: 'PUT',
			credentials: 'include',
			headers: authorizationHeaders(input.token),
			body: JSON.stringify(input.policy),
		},
	);
	if (response.ok) return;
	let serverCode: string | undefined;
	try {
		const body = (await response.json()) as {
			error?: { code?: unknown };
		} | null;
		if (body && typeof body === 'object' && body.error) {
			const code = body.error.code;
			if (typeof code === 'string') serverCode = code;
		}
	} catch {
		// A non-JSON error body carries no structured code; fall through.
	}
	const options = {
		status: response.status,
		...(serverCode === undefined ? {} : { cause: { serverCode } }),
	};
	if (response.status === 401 || response.status === 403) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.Unauthorized,
			serverCode ?? 'request was not authorized',
			options,
		);
	}
	throw new BrowserSyncError(
		BrowserSyncErrorCode.RequestFailed,
		serverCode ?? `request failed with status ${response.status}`,
		options,
	);
}

/**
 * True when a policy upload was rejected because the workspace access revision
 * moved under us. The server reports this as `sync.policy_revision_changed`
 * (409); the caller should re-read access-state once and retry.
 */
export function isPolicyRevisionChanged(error: unknown): boolean {
	return (
		error instanceof BrowserSyncError &&
		error.status === 409 &&
		(error.cause as { serverCode?: unknown } | undefined)?.serverCode ===
			'sync.policy_revision_changed'
	);
}
