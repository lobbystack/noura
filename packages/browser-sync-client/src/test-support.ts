/** Shared fakes and fixtures for the browser sync client tests. Not exported from the package. */

import {
	createMemoryBindingStore,
	createMemoryKeyStore,
	encodeBase64,
	wrapKey,
	type AccessPolicy,
	type BrowserSyncBindingRecord,
	type DeviceIdentity,
	type FetchLike,
	type WebKeyEnvelope,
} from '@noura/browser-sync';
import {
	createMemorySyncStateStore,
	MemorySyncStorage,
} from '@noura/browser-sync-engine';
import type { BrowserWorkspaceFiles } from '@noura/browser-workspace';
import type { EncryptedOperation } from '@noura/shared';
import { createBrowserSyncController } from './controller';
import type { BrowserSyncWorkspaceBinding } from './types';

export const PASSPHRASE = 'correct horse battery staple';
export const ORIGIN = 'https://sync.example';
export const encoder = new TextEncoder();

export function json(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' },
	});
}

export function requestUrl(input: RequestInfo | URL): string {
	if (typeof input === 'string') return input;
	if (input instanceof URL) return input.toString();
	return input.url;
}

export function enrollingFetch(token = 'token-one'): FetchLike {
	return async (input) => {
		const url = requestUrl(input);
		if (url.endsWith('/v1/device-challenges'))
			return json({
				challenge: 'challenge-one',
				accountId: 'acct_one',
				expiresIn: 300,
			});
		if (url.endsWith('/v1/devices')) return json({ token });
		throw new Error(`unexpected request in test: ${url}`);
	};
}

export async function enrolledController(token = 'token-one') {
	const keyStore = createMemoryKeyStore();
	const controller = await createBrowserSyncController({
		keyStore,
		origin: ORIGIN,
		fetch: enrollingFetch(token),
	});
	const result = await controller.enroll({ passphrase: PASSPHRASE });
	if (!result.ok)
		throw new Error(`enrollment failed in test: ${result.message}`);
	return { controller, keyStore };
}

export function binding(
	overrides: Partial<BrowserSyncWorkspaceBinding> = {},
): BrowserSyncWorkspaceBinding {
	return {
		workspaceId: 'ws_test',
		objectId: 'obj_test',
		epoch: 1,
		policyRevision: '1',
		objectKeys: new Map(),
		pinnedSigners: new Map(),
		storage: new MemorySyncStorage(),
		state: createMemorySyncStateStore(),
		remote: {
			async push(operations) {
				return { sequences: operations.map((_, index) => String(index + 1)) };
			},
			async pull(cursor) {
				return { accessRevision: '1', operations: [], cursor, hasMore: false };
			},
		},
		...overrides,
	};
}

export function syncFetch(token = 'token-bootstrap'): FetchLike {
	return async (input, init) => {
		const url = requestUrl(input);
		const method = init?.method ?? 'GET';
		if (url.endsWith('/v1/device-challenges'))
			return json({
				challenge: 'challenge-bootstrap',
				accountId: 'acct_one',
				expiresIn: 300,
			});
		if (url.endsWith('/v1/devices')) return json({ token });
		if (url.endsWith('/v1/workspaces') && method === 'POST')
			return json({ id: 'ws' }, 201);
		if (/\/v1\/workspaces\/[^/]+\/objects$/.test(url) && method === 'POST')
			return json({ epoch: 1 });
		if (/\/v1\/workspaces\/[^/]+\/access$/.test(url) && method === 'PUT')
			return json({ ok: true });
		if (url.endsWith('/operations') && method === 'POST')
			return json({ sequences: ['1'] });
		if (url.includes('/operations?') && method === 'GET')
			return json({
				accessRevision: '1',
				operations: [],
				cursor: '1',
				hasMore: false,
			});
		if (url.includes('/keys?') && method === 'GET')
			return json({ envelopes: [], hasMore: false });
		if (url.endsWith('/access-state') && method === 'GET')
			return json({
				revision: '1',
				members: [],
				objects: [],
				envelopes: [],
				devices: [],
				policy: null,
			});
		throw new Error(`unexpected sync request in test: ${method} ${url}`);
	};
}

export async function sha256Hex(bytes: Uint8Array): Promise<string> {
	const digest = await crypto.subtle.digest(
		'SHA-256',
		bytes.buffer.slice(
			bytes.byteOffset,
			bytes.byteOffset + bytes.byteLength,
		) as ArrayBuffer,
	);
	return Array.from(new Uint8Array(digest), (byte) =>
		byte.toString(16).padStart(2, '0'),
	).join('');
}

/**
 * An in-memory `BrowserWorkspaceFiles` with 64-hex content revisions, matching
 * the real `BrowserWorkspaceStorage` BLAKE3 revisions that the sync file-change
 * schema requires for a non-null `baseRevision`.
 */
export function workspaceFiles(
	initial: Record<string, Uint8Array>,
	objects: Array<{ id: string; path: string; type?: string }> = [],
): BrowserWorkspaceFiles {
	const files = new Map<string, Uint8Array>();
	for (const [path, bytes] of Object.entries(initial)) {
		files.set(path, bytes.slice());
	}
	return {
		list: async () => [...files.keys()].sort(),
		listObjects: async () =>
			objects.map((entry) => ({
				id: entry.id,
				path: entry.path,
				type: entry.type ?? 'note',
			})),
		async read(path) {
			const bytes = files.get(path);
			return bytes === undefined
				? null
				: { bytes: bytes.slice(), revision: await sha256Hex(bytes) };
		},
		async write(input) {
			const current = files.get(input.path);
			if (input.expectedRevision === null && current !== undefined)
				throw new Error(`path exists: ${input.path}`);
			if (
				typeof input.expectedRevision === 'string' &&
				(current === undefined ||
					(await sha256Hex(current)) !== input.expectedRevision)
			)
				throw new Error(`stale revision: ${input.path}`);
			files.set(input.path, input.bytes.slice());
			return { path: input.path, revision: await sha256Hex(input.bytes) };
		},
		async move(input) {
			const current = files.get(input.from);
			if (current === undefined) throw new Error(`not found: ${input.from}`);
			if ((await sha256Hex(current)) !== input.expectedRevision)
				throw new Error(`stale revision: ${input.from}`);
			if (files.has(input.to)) throw new Error(`path exists: ${input.to}`);
			files.set(input.to, current.slice());
			files.delete(input.from);
			return { path: input.to, revision: await sha256Hex(current) };
		},
		async delete(input) {
			const current = files.get(input.path);
			if (current === undefined) {
				if (input.expectedRevision === null) return;
				throw new Error(`not found: ${input.path}`);
			}
			if (
				typeof input.expectedRevision === 'string' &&
				(await sha256Hex(current)) !== input.expectedRevision
			)
				throw new Error(`stale revision: ${input.path}`);
			files.delete(input.path);
		},
	};
}

export interface SyncServerState {
	createdWorkspaces: string[];
	createdObjects: string[];
	policies: Array<{
		revision?: string;
		objects?: Array<{ objectId: string; envelopes: unknown[] }>;
	}>;
	/** Latest committed signed access policy, returned from access-state. */
	policy: AccessPolicy | null;
	/** Workspace access revision as a canonical decimal string. */
	accessRevision: string;
	devices: Array<Record<string, unknown>>;
	keys: Array<{ workspaceId: string; envelope: Record<string, unknown> }>;
	operations: Array<{
		workspaceId: string;
		operation: EncryptedOperation;
		sequence: string;
	}>;
	pushes: EncryptedOperation[][];
	nextSequence: number;
}

export function createSyncServerState(): SyncServerState {
	return {
		createdWorkspaces: [],
		createdObjects: [],
		policies: [],
		policy: null,
		accessRevision: '0',
		devices: [],
		keys: [],
		operations: [],
		pushes: [],
		nextSequence: 0,
	};
}

export function randomBytes(length: number): Uint8Array {
	return crypto.getRandomValues(new Uint8Array(length));
}

export function recordingSyncFetch(
	state: SyncServerState,
	token = 'token-objects',
): FetchLike {
	return async (input, init) => {
		const url = requestUrl(input);
		const method = init?.method ?? 'GET';
		const parsed = new URL(url, ORIGIN);
		const pathname = parsed.pathname;
		if (pathname === '/v1/device-challenges')
			return json({
				challenge: 'challenge-objects',
				accountId: 'acct_one',
				expiresIn: 300,
			});
		if (pathname === '/v1/devices') return json({ token });
		if (pathname === '/v1/workspaces' && method === 'POST') {
			const body = JSON.parse(String(init?.body ?? '{}')) as { id: string };
			state.createdWorkspaces.push(body.id);
			return json({ id: body.id }, 201);
		}
		const objectMatch = pathname.match(/^\/v1\/workspaces\/([^/]+)\/objects$/);
		if (objectMatch && method === 'POST') {
			const body = JSON.parse(String(init?.body ?? '{}')) as { id: string };
			state.createdObjects.push(body.id);
			return json({ epoch: state.createdObjects.length });
		}
		const accessMatch = pathname.match(/^\/v1\/workspaces\/([^/]+)\/access$/);
		if (accessMatch && method === 'PUT') {
			const policy = JSON.parse(String(init?.body ?? '{}')) as AccessPolicy;
			const next = BigInt(policy.revision);
			const current = BigInt(state.accessRevision);
			if (next === current) {
				if (state.policy && state.policy.signature === policy.signature)
					return json({ ok: true });
				return json({ error: { code: 'sync.policy_revision_changed' } }, 409);
			}
			if (next !== current + 1n)
				return json({ error: { code: 'sync.policy_revision_changed' } }, 409);
			state.policies.push(policy);
			state.policy = policy;
			state.accessRevision = policy.revision;
			return json({ ok: true });
		}
		if (pathname.endsWith('/access-state') && method === 'GET') {
			return json({
				revision: state.accessRevision,
				members: [{ accountId: 'acct_one', role: 'owner' }],
				objects: [],
				envelopes: [],
				devices: state.devices,
				policy: state.policy,
			});
		}
		const keysMatch = pathname.match(/^\/v1\/workspaces\/([^/]+)\/keys$/);
		if (keysMatch && method === 'GET') {
			return json({
				envelopes: state.keys
					.filter((entry) => entry.workspaceId === keysMatch[1])
					.map((entry) => entry.envelope),
				hasMore: false,
			});
		}
		const operationsMatch = pathname.match(
			/^\/v1\/workspaces\/([^/]+)\/operations$/,
		);
		if (operationsMatch && method === 'POST') {
			const body = JSON.parse(String(init?.body ?? '{}')) as {
				operations: EncryptedOperation[];
			};
			state.pushes.push(body.operations);
			return json({
				sequences: body.operations.map(() => String(++state.nextSequence)),
			});
		}
		if (operationsMatch && method === 'GET') {
			const after = BigInt(parsed.searchParams.get('after') ?? '0');
			const operations = state.operations
				.filter(
					(entry) =>
						entry.workspaceId === operationsMatch[1] &&
						BigInt(entry.sequence) > after,
				)
				.map((entry) => ({ ...entry.operation, sequence: entry.sequence }));
			const cursor =
				operations.length > 0
					? operations[operations.length - 1]!.sequence
					: String(after);
			return json({
				accessRevision: '1',
				operations,
				cursor,
				hasMore: false,
			});
		}
		throw new Error(`unexpected sync request in test: ${method} ${pathname}`);
	};
}

export async function deliveredKeyFor(input: {
	state: SyncServerState;
	workspaceId: string;
	objectId: string;
	epoch: number;
	identity: DeviceIdentity;
	key: Uint8Array;
}): Promise<void> {
	const envelope = await wrapKey({
		workspace_id: input.workspaceId,
		object_id: input.objectId,
		epoch: input.epoch,
		signing_device: input.identity.deviceId,
		device_id: input.identity.deviceId,
		signing_secret: encodeBase64(input.identity.signingSeed),
		recipient_public: encodeBase64(input.identity.x25519Public),
		object_key: encodeBase64(input.key),
		ephemeral_secret: encodeBase64(randomBytes(32)),
		salt: encodeBase64(randomBytes(32)),
		nonce: encodeBase64(randomBytes(12)),
	});
	input.state.keys.push({
		workspaceId: input.workspaceId,
		envelope: {
			objectId: input.objectId,
			epoch: input.epoch,
			deviceId: input.identity.deviceId,
			signingDevice: input.identity.deviceId,
			recipientPublicKey: envelope.recipient_public_key,
			ephemeralPublicKey: envelope.ephemeral_public_key,
			salt: envelope.salt,
			nonce: envelope.nonce,
			wrappedKey: envelope.wrapped_key,
			signature: envelope.signature,
			construction: 'web',
		},
	});
}

export function envelopeFor(
	record: BrowserSyncBindingRecord,
	objectId: string,
): WebKeyEnvelope {
	const bound = record.objects[objectId];
	if (!bound) throw new Error(`no bound object ${objectId}`);
	return {
		workspace_id: record.workspaceId,
		object_id: objectId,
		epoch: bound.epoch,
		signing_device: bound.key.deviceId,
		device_id: bound.key.deviceId,
		recipient_public_key: bound.key.recipientPublicKey,
		ephemeral_public_key: bound.key.ephemeralPublicKey,
		salt: bound.key.salt,
		nonce: bound.key.nonce,
		wrapped_key: bound.key.wrappedKey,
		signature: bound.key.signature,
	};
}

export function accessDigestVector(): AccessPolicy {
	const signature = encodeBase64(
		Uint8Array.from({ length: 64 }, (_, index) => index),
	);
	return {
		version: 1,
		workspaceId: 'ws_vector',
		revision: '7',
		previousPolicyDigest: null,
		deviceId: 'device_vector',
		members: [{ accountId: 'account_vector', role: 'owner' }],
		objects: [
			{
				objectId: 'object_vector',
				epoch: 2,
				grants: [],
				envelopes: [
					{
						deviceId: 'device_vector',
						wrappedKey: encodeBase64(new Uint8Array([1, 2, 3, 4])),
						signature,
						construction: 'web',
						recipientPublicKey: encodeBase64(new Uint8Array(32)),
						ephemeralPublicKey: encodeBase64(new Uint8Array(32)),
						salt: encodeBase64(new Uint8Array(32)),
						nonce: encodeBase64(new Uint8Array(12)),
					},
				],
			},
		],
		signature,
	};
}

export async function bootstrapPerObject(options: {
	files: BrowserWorkspaceFiles;
	bindingStore: ReturnType<typeof createMemoryBindingStore>;
	stateStore: ReturnType<typeof createMemorySyncStateStore>;
	fetch: FetchLike;
}) {
	const keyStore = createMemoryKeyStore();
	const controller = await createBrowserSyncController({
		keyStore,
		origin: ORIGIN,
		fetch: options.fetch,
		bindingStore: options.bindingStore,
		stateStore: options.stateStore,
	});
	await controller.enroll({ passphrase: PASSPHRASE });
	const enabled = await controller.enableSync({
		workspaceId: 'workspace_local',
		workspaceFiles: options.files,
	});
	if (!enabled.ok) throw new Error(`enable failed: ${enabled.message}`);
	const record = await options.bindingStore.read();
	if (!record) throw new Error('binding was not persisted');
	return { controller, keyStore, record };
}
