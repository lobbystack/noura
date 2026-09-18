import { describe, expect, test } from 'bun:test';
import nativeFixture from '../../../../docs/workspace-format/fixtures/native-recovery-v1.json';
import type { EncryptedOperation, SequencedOperation } from '@noura/shared';
import {
	accessDigest,
	accessSigningBytes,
	buildAttachmentFileChange,
	BrowserSyncErrorCode,
	createDeviceIdentity,
	createFileChangeCodec,
	createMemoryKeyStore,
	decodeBase64,
	deviceFingerprintForCard,
	encryptAttachment,
	encodeBase64,
	encodeFileChange,
	encodeRecipient,
	sealOperation,
	unlockDeviceIdentity,
	unwrapKey,
	wrapKey,
	type AccessPolicy,
	type DeviceIdentity,
	type FetchLike,
	type NativeRecoveryObject,
	type WebKeyEnvelope,
} from '@noura/browser-sync';
import type { BrowserWorkspaceFiles } from '@noura/browser-workspace';
import {
	BrowserSyncEngineErrorCode,
	MemorySyncStorage,
	createEmptySyncState,
	createMemorySyncStateStore,
	type BrowserSyncRemote,
	type WorkspaceStorageLike,
} from '@noura/browser-sync-engine';
import {
	DEFAULT_BROWSER_SYNC_BUNDLE_ID,
	MAX_BROWSER_ATTACHMENT_BYTES,
	createBrowserAttachmentFetcher,
	createBrowserSyncController,
	createBrowserSyncWorkspaceBinding,
	createMemoryBindingStore,
	createSameOriginFetch,
	extractEmbeddedRecoveryIdentity,
	runBrowserSyncReconcile,
	runBrowserSyncSendAttachment,
	type BrowserSyncBindingRecord,
	type BrowserSyncWorkspaceBinding,
} from './browser-sync';

const PASSPHRASE = 'correct horse battery staple';
const ORIGIN = 'https://sync.example';
const encoder = new TextEncoder();

function json(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' },
	});
}

function requestUrl(input: RequestInfo | URL): string {
	if (typeof input === 'string') return input;
	if (input instanceof URL) return input.toString();
	return input.url;
}

function enrollingFetch(token = 'token-one'): FetchLike {
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

async function enrolledController(token = 'token-one') {
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

function binding(
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

describe('browser sync controller custody', () => {
	test('reports unavailable without OPFS and refuses to enroll or sync', async () => {
		const controller = await createBrowserSyncController({ keyStore: null });
		expect(controller.status()).toBe('unavailable');

		const enroll = await controller.enroll({ passphrase: PASSPHRASE });
		expect(enroll.ok).toBe(false);
		if (!enroll.ok) expect(enroll.code).toBe('unavailable');
		expect(controller.device()).toBeNull();

		const sync = await controller.syncNow();
		expect(sync.status).toBe('unavailable');
	});

	test('transitions locked -> enrolled -> locked -> unlocked and persists only wrapped custody', async () => {
		const keyStore = createMemoryKeyStore();
		const controller = await createBrowserSyncController({
			keyStore,
			origin: ORIGIN,
			fetch: enrollingFetch('token-secret'),
		});
		expect(controller.status()).toBe('locked');
		expect(controller.device()).toBeNull();

		const enroll = await controller.enroll({ passphrase: PASSPHRASE });
		expect(enroll.ok).toBe(true);
		expect(controller.status()).toBe('enrolled');
		const device = controller.device();
		expect(device?.enrolled).toBe(true);

		const stored = await keyStore.read(DEFAULT_BROWSER_SYNC_BUNDLE_ID);
		expect(stored).toBeDefined();
		expect(stored?.ciphertext).not.toContain('token-secret');
		expect(JSON.stringify(stored)).not.toContain(PASSPHRASE);
		expect(JSON.stringify(stored)).not.toContain('signingSeed');

		controller.lock();
		expect(controller.status()).toBe('locked');

		const wrong = await controller.unlock({ passphrase: 'not the passphrase' });
		expect(wrong.ok).toBe(false);
		if (!wrong.ok) expect(wrong.code).toBe('passphrase_rejected');
		expect(controller.status()).toBe('error');

		const unlocked = await controller.unlock({ passphrase: PASSPHRASE });
		expect(unlocked.ok).toBe(true);
		expect(controller.status()).toBe('enrolled');
		expect(controller.device()?.deviceId).toBe(device?.deviceId);
	});

	test('a second controller on the same key store starts locked with the same device id', async () => {
		const { controller, keyStore } = await enrolledController();
		const deviceId = controller.device()?.deviceId;
		const reopened = await createBrowserSyncController({
			keyStore,
			origin: ORIGIN,
			fetch: enrollingFetch(),
		});
		expect(reopened.status()).toBe('locked');
		expect(reopened.device()?.deviceId).toBe(deviceId);
		expect(reopened.device()?.enrolled).toBe(false);
	});

	test('a failed enrollment leaves the previous wrapped bundle untouched', async () => {
		const keyStore = createMemoryKeyStore();
		const first = await createBrowserSyncController({
			keyStore,
			origin: ORIGIN,
			fetch: enrollingFetch('token-keep'),
		});
		await first.enroll({ passphrase: PASSPHRASE });
		const before = await keyStore.read(DEFAULT_BROWSER_SYNC_BUNDLE_ID);

		const failing = await createBrowserSyncController({
			keyStore,
			origin: ORIGIN,
			fetch: async () => json({ error: 'rejected' }, 400),
		});
		expect(failing.status()).toBe('locked');
		const result = await failing.enroll({ passphrase: PASSPHRASE });
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.code).toBe('enroll_failed');
		expect(failing.status()).toBe('error');

		const after = await keyStore.read(DEFAULT_BROWSER_SYNC_BUNDLE_ID);
		expect(after).toEqual(before);
	});

	test('syncNow returns locked before unlock and not_configured without object keys', async () => {
		const { controller } = await enrolledController();
		controller.lock();

		const locked = await controller.syncNow();
		expect(locked.status).toBe('locked');
		await controller.unlock({ passphrase: PASSPHRASE });

		const noBinding = await controller.syncNow();
		expect(noBinding.status).toBe('not_configured');

		controller.setBinding(binding({ objectKeys: new Map() }));
		const noKeys = await controller.syncNow();
		expect(noKeys.status).toBe('not_configured');

		const summary = await controller.workspaceSummary();
		expect(summary.configured).toBe(true);
		expect(summary.pending).toBe(0);
		expect(summary.conflicts).toBe(0);
	});

	test('same-origin fetch rejects cross-origin requests', async () => {
		const seen: string[] = [];
		const sameOrigin = createSameOriginFetch(ORIGIN, async (input) => {
			seen.push(requestUrl(input));
			return json({ ok: true });
		});
		await sameOrigin('/v1/device-challenges');
		expect(seen).toEqual(['/v1/device-challenges']);
		await expect(
			sameOrigin('https://other.example/v1/device-challenges'),
		).rejects.toThrow('same-origin');
	});

	test('same-origin fetch forces redirect: error', async () => {
		let captured: RequestInit | undefined;
		const sameOrigin = createSameOriginFetch(ORIGIN, async (_input, init) => {
			captured = init;
			return json({ ok: true });
		});
		await sameOrigin('/v1/device-challenges');
		expect(captured?.redirect).toBe('error');
	});
});

describe('browser sync reconciliation', () => {
	test('pushes local changes and applies remote operations with in-memory fakes', async () => {
		const passphrase = PASSPHRASE;
		const identity = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase }),
			passphrase,
		);
		// The remote operation is authored by another device; the local identity
		// must not match it or the engine would skip it as its own operation.
		const remoteIdentity = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase }),
			passphrase,
		);
		const objectKey = crypto.getRandomValues(new Uint8Array(32));
		const objectKeys = new Map([['obj_reconcile', objectKey]]);
		const pinnedSigners = new Map([
			[remoteIdentity.deviceId, remoteIdentity.signingPublic],
		]);

		const remoteCodec = createFileChangeCodec({
			identity: remoteIdentity,
			objectKeys,
			pinnedSigners: new Map([
				[remoteIdentity.deviceId, encodeBase64(remoteIdentity.signingPublic)],
			]),
		});
		const remoteOperation: EncryptedOperation =
			await remoteCodec.sealFileChange({
				workspaceId: 'ws_reconcile',
				objectId: 'obj_reconcile',
				epoch: 1,
				policyRevision: '1',
				change: {
					path: 'remote/created.md',
					previousPath: null,
					baseRevision: null,
					content: encoder.encode('# Remote\n'),
				},
			});

		const pushedBatches: EncryptedOperation[][] = [];
		const remote: BrowserSyncRemote = {
			async push(operations) {
				pushedBatches.push(operations);
				return { sequences: operations.map((_, index) => String(index + 1)) };
			},
			async pull(cursor) {
				if (cursor === '0') {
					const operation: SequencedOperation = {
						...remoteOperation,
						sequence: '1',
					};
					return {
						accessRevision: '1',
						operations: [operation],
						cursor: '1',
						hasMore: false,
					};
				}
				return { accessRevision: '1', operations: [], cursor, hasMore: false };
			},
		};

		const storage = new MemorySyncStorage({
			'notes/local.md': encoder.encode('# Local\n'),
		});
		const state = createMemorySyncStateStore();

		const result = await runBrowserSyncReconcile({
			identity,
			workspaceId: 'ws_reconcile',
			objectId: 'obj_reconcile',
			epoch: 1,
			policyRevision: '1',
			objectKeys,
			pinnedSigners,
			storage,
			state,
			remote,
			now: () => 1_700_000_000_000,
		});

		expect(result.pushed).toBe(1);
		expect(result.applied).toBe(1);
		expect(result.conflicts).toHaveLength(0);
		expect(result.cursor).toBe('1');
		expect(pushedBatches).toHaveLength(1);
		expect(pushedBatches[0]).toHaveLength(1);

		expect(await storage.read('notes/local.md')).not.toBeNull();
		expect(await storage.read('remote/created.md')).not.toBeNull();

		const persisted = await state.read();
		expect(persisted.pushedRevisions['notes/local.md']).toBeDefined();
		expect(persisted.pushedRevisions['remote/created.md']).toBeDefined();
		expect(persisted.outbox).toHaveLength(0);
	});

	test('syncNow reconciles through a bound workspace with transport and storage fakes', async () => {
		const { controller } = await enrolledController('token-binding');
		const workspaceId = 'ws_binding';
		const objectId = 'obj_binding';
		const objectKey = crypto.getRandomValues(new Uint8Array(32));
		const objectKeys = new Map([[objectId, objectKey]]);

		// A separate, pinned signer authors the remote operation.
		const remoteIdentity = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
		const pinnedSigners = new Map([
			[remoteIdentity.deviceId, remoteIdentity.signingPublic],
		]);
		const remoteCodec = createFileChangeCodec({
			identity: remoteIdentity,
			objectKeys,
			pinnedSigners: new Map([
				[remoteIdentity.deviceId, encodeBase64(remoteIdentity.signingPublic)],
			]),
		});
		const remoteOperation = await remoteCodec.sealFileChange({
			workspaceId,
			objectId,
			epoch: 1,
			policyRevision: '1',
			change: {
				path: 'remote/pulled.md',
				previousPath: null,
				baseRevision: null,
				content: encoder.encode('# Pulled\n'),
			},
		});

		const memory = new MemorySyncStorage({
			'notes/local.md': encoder.encode('# Local\n'),
		});
		const workspace: WorkspaceStorageLike = {
			read: (path) => memory.read(path),
			async write(input) {
				const result = await memory.write({
					path: input.path,
					bytes: input.bytes,
					expectedRevision: input.expectedRevision ?? null,
				});
				return { revision: result.revision };
			},
			move: (input) =>
				memory.move({
					from: input.from,
					to: input.to,
					expectedRevision: input.expectedRevision ?? null,
				}),
			delete: (input) =>
				memory.delete({
					path: input.path,
					expectedRevision: input.expectedRevision ?? null,
				}),
			async rebuild() {
				return {
					files: (await memory.list()).map((path) => ({ path })),
					managed: [],
				};
			},
		};

		const transportFetch: FetchLike = async (input, init) => {
			const url = requestUrl(input);
			if (init?.method === 'POST' && url.endsWith('/operations')) {
				return json({ sequences: ['1'] });
			}
			if (init?.method === 'GET' && url.includes('/operations?')) {
				return json({
					accessRevision: '1',
					operations: [{ ...remoteOperation, sequence: '1' }],
					cursor: '1',
					hasMore: false,
				});
			}
			throw new Error(`unexpected transport request in test: ${url}`);
		};

		const bound = await createBrowserSyncWorkspaceBinding({
			workspaceId,
			objectId,
			epoch: 1,
			policyRevision: '1',
			objectKeys,
			pinnedSigners,
			workspace,
			origin: ORIGIN,
			token: 'token-binding',
			fetch: transportFetch,
			state: createMemorySyncStateStore(),
		});
		expect(bound).not.toBeNull();
		controller.setBinding(bound);

		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('synced');
		if (outcome.status === 'synced') {
			expect(outcome.pushed).toBe(1);
			expect(outcome.applied).toBe(1);
			expect(outcome.conflicts).toBe(0);
			expect(outcome.cursor).toBe('1');
		}
		expect(await memory.read('remote/pulled.md')).not.toBeNull();
	});
});

function syncFetch(token = 'token-bootstrap'): FetchLike {
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

async function sha256Hex(bytes: Uint8Array): Promise<string> {
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
function workspaceFiles(
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

describe('browser sync enablement', () => {
	test('syncNow returns not_configured without a binding', async () => {
		const controller = await createBrowserSyncController({
			keyStore: createMemoryKeyStore(),
			origin: ORIGIN,
			fetch: syncFetch(),
		});
		await controller.enroll({ passphrase: PASSPHRASE });
		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('not_configured');
	});

	test('enableSync bootstraps a binding, persists it, and syncNow reconciles', async () => {
		const bindingStore = createMemoryBindingStore();
		const stateStore = createMemorySyncStateStore();
		const controller = await createBrowserSyncController({
			keyStore: createMemoryKeyStore(),
			origin: ORIGIN,
			fetch: syncFetch(),
			bindingStore,
			stateStore,
		});
		await controller.enroll({ passphrase: PASSPHRASE });
		const files = workspaceFiles(
			{
				'notes/local.md': encoder.encode('# Local\n'),
				'.noura/workspace.yaml': encoder.encode('{"id":"workspace_local"}'),
			},
			[{ id: 'note_local', path: 'notes/local.md' }],
		);

		const enabled = await controller.enableSync({
			workspaceId: 'workspace_local',
			workspaceFiles: files,
		});
		expect(enabled.ok).toBe(true);
		if (enabled.ok) expect(enabled.value.configured).toBe(true);

		const record = await bindingStore.read();
		expect(record).not.toBeNull();
		expect(record?.localWorkspaceId).toBe('workspace_local');
		expect(record?.revision).toBe('1');
		expect(record?.objects[record.objectId]?.epoch).toBe(1);
		// The persisted key is a wrapped envelope, not the raw object key.
		expect(record?.objects[record.objectId]?.key.construction).toBe('web');
		expect(
			record?.objects[record.objectId]?.key.wrappedKey.length,
		).toBeGreaterThan(0);
		expect(record?.pinnedSigners).toBeDefined();

		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('synced');
		if (outcome.status === 'synced') {
			expect(outcome.pushed).toBe(1);
			expect(outcome.applied).toBe(0);
			expect(outcome.conflicts).toBe(0);
			expect(outcome.cursor).toBe('1');
		}

		const summary = await controller.workspaceSummary();
		expect(summary.configured).toBe(true);
		expect(summary.cursor).toBe('1');
		expect(summary.pending).toBe(0);
	});

	test('enableSync reuses a durable binding instead of creating a second workspace', async () => {
		const bindingStore = createMemoryBindingStore();
		const stateStore = createMemorySyncStateStore();
		let createdWorkspaces = 0;
		const base = syncFetch();
		const fetchImpl: FetchLike = async (input, init) => {
			const url = requestUrl(input);
			if (init?.method === 'POST' && url.endsWith('/v1/workspaces'))
				createdWorkspaces += 1;
			return base(input, init);
		};
		const controller = await createBrowserSyncController({
			keyStore: createMemoryKeyStore(),
			origin: ORIGIN,
			fetch: fetchImpl,
			bindingStore,
			stateStore,
		});
		await controller.enroll({ passphrase: PASSPHRASE });
		const files = workspaceFiles(
			{
				'notes/local.md': encoder.encode('# Local\n'),
			},
			[{ id: 'note_local', path: 'notes/local.md' }],
		);

		const first = await controller.enableSync({
			workspaceId: 'workspace_local',
			workspaceFiles: files,
		});
		expect(first.ok).toBe(true);
		expect(createdWorkspaces).toBe(1);

		const resumed = await controller.resumeSync({
			workspaceId: 'workspace_local',
			workspaceFiles: files,
		});
		expect(resumed.ok).toBe(true);
		expect(createdWorkspaces).toBe(1);
	});

	test('resumeSync reports not_configured when no binding exists', async () => {
		const controller = await createBrowserSyncController({
			keyStore: createMemoryKeyStore(),
			origin: ORIGIN,
			fetch: syncFetch(),
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
		});
		await controller.enroll({ passphrase: PASSPHRASE });
		const resumed = await controller.resumeSync({
			workspaceId: 'workspace_local',
			workspaceFiles: workspaceFiles({}),
		});
		expect(resumed.ok).toBe(false);
		if (!resumed.ok) expect(resumed.code).toBe('not_configured');
	});
});

interface SyncServerState {
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

function createSyncServerState(): SyncServerState {
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

function randomBytes(length: number): Uint8Array {
	return crypto.getRandomValues(new Uint8Array(length));
}

function recordingSyncFetch(
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

async function deliveredKeyFor(input: {
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

function envelopeFor(
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

function accessDigestVector(): AccessPolicy {
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

async function bootstrapPerObject(options: {
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

describe('browser sync per-object binding', () => {
	test('bootstrap creates one object and key per managed object with one policy', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const files = workspaceFiles(
			{
				'notes/a.md': encoder.encode('# A\n'),
				'tasks/b.md': encoder.encode('# B\n'),
			},
			[
				{ id: 'note_a', path: 'notes/a.md' },
				{ id: 'task_b', path: 'tasks/b.md', type: 'task' },
			],
		);
		const { record } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});

		expect(state.createdObjects).toHaveLength(2);
		expect(state.policies).toHaveLength(1);
		expect(state.policies[0]?.objects).toHaveLength(2);
		for (const object of state.policies[0]?.objects ?? []) {
			expect(object.envelopes).toHaveLength(1);
		}
		expect(Object.keys(record.objects)).toHaveLength(2);
		for (const bound of Object.values(record.objects)) {
			expect(bound.key.construction).toBe('web');
			expect(bound.key.wrappedKey.length).toBeGreaterThan(0);
		}
	});

	test('seals each change under the object that owns its path', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const files = workspaceFiles(
			{
				'notes/a.md': encoder.encode('# A\n'),
				'tasks/b.md': encoder.encode('# B\n'),
			},
			[
				{ id: 'note_a', path: 'notes/a.md' },
				{ id: 'task_b', path: 'tasks/b.md', type: 'task' },
			],
		);
		const { controller, record } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		await controller.syncNow();

		const taskObjectId = Object.entries(record.objects).find(
			([, bound]) => bound.path === 'tasks/b.md',
		)?.[0];
		expect(taskObjectId).toBeDefined();

		const current = await files.read('tasks/b.md');
		await files.write({
			path: 'tasks/b.md',
			bytes: encoder.encode('# B changed\n'),
			expectedRevision: current?.revision ?? null,
		});
		state.pushes = [];
		const outcome = await controller.syncNow();
		if (outcome.status !== 'synced') throw new Error(JSON.stringify(outcome));
		expect(outcome.status).toBe('synced');
		const pushed = state.pushes.flat();
		expect(pushed).toHaveLength(1);
		expect(pushed[0]?.objectId).toBe(taskObjectId!);
	});

	test('skips and counts local files that no object owns', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const files = workspaceFiles(
			{
				'notes/a.md': encoder.encode('# A\n'),
				'assets/blob.bin': new Uint8Array([1, 2, 3]),
			},
			[{ id: 'note_a', path: 'notes/a.md' }],
		);
		const { controller } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});

		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('synced');
		if (outcome.status === 'synced') {
			expect(outcome.skippedUnmanaged).toBe(1);
			expect(outcome.pushed).toBe(1);
		}
	});

	test('approveDevice rejects a fingerprint mismatch and persists an approval', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const remote = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
		const remoteRecipient = encodeRecipient(remote.x25519Public);
		state.devices = [
			{
				deviceId: remote.deviceId,
				accountId: 'acct_one',
				publicKey: encodeBase64(remote.signingPublic),
				encryptionRecipient: remoteRecipient,
			},
		];
		const { controller } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});

		const listed = await controller.listWorkspaceDevices();
		expect(listed.ok).toBe(true);
		if (!listed.ok) throw new Error('listing failed');
		const card = listed.value.find(
			(entry) => entry.deviceId === remote.deviceId,
		);
		expect(card).toBeDefined();
		expect(card?.fingerprint).toBe(
			deviceFingerprintForCard(
				remote.deviceId,
				'acct_one',
				remote.signingPublic,
				remoteRecipient,
			),
		);

		const mismatch = await controller.approveDevice(
			remote.deviceId,
			'not-the-fingerprint',
		);
		expect(mismatch.ok).toBe(false);
		if (!mismatch.ok) expect(mismatch.code).toBe('fingerprint_mismatch');
		expect(
			(await bindingStore.read())?.pinnedSigners[remote.deviceId],
		).toBeUndefined();

		const approved = await controller.approveDevice(
			remote.deviceId,
			card?.fingerprint ?? '',
		);
		expect(approved.ok).toBe(true);
		expect((await bindingStore.read())?.pinnedSigners[remote.deviceId]).toBe(
			encodeBase64(remote.signingPublic),
		);

		const revoked = await controller.revokeDeviceApproval(remote.deviceId);
		expect(revoked.ok && revoked.value).toBe(true);
		expect(
			(await bindingStore.read())?.pinnedSigners[remote.deviceId],
		).toBeUndefined();
	});

	test('receiveKeys merges a delivered key before reconcile', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const { controller, keyStore, record } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		const bundle = await keyStore.read(DEFAULT_BROWSER_SYNC_BUNDLE_ID);
		if (!bundle) throw new Error('no stored bundle');
		const identity = await unlockDeviceIdentity(bundle, PASSPHRASE);
		// The delivered operation is authored by another device; approve its signer
		// so the engine applies it rather than skipping it as this device's own.
		const remote = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
		const remoteRecipient = encodeRecipient(remote.x25519Public);
		state.devices = [
			{
				deviceId: remote.deviceId,
				accountId: 'acct_one',
				publicKey: encodeBase64(remote.signingPublic),
				encryptionRecipient: remoteRecipient,
			},
		];
		const approved = await controller.approveDevice(
			remote.deviceId,
			deviceFingerprintForCard(
				remote.deviceId,
				'acct_one',
				remote.signingPublic,
				remoteRecipient,
			),
		);
		if (!approved.ok) throw new Error(approved.message);

		const deliveredKey = randomBytes(32);
		await deliveredKeyFor({
			state,
			workspaceId: record.workspaceId,
			objectId: record.objectId,
			epoch: 1,
			identity,
			key: deliveredKey,
		});
		const codec = createFileChangeCodec({
			identity: remote,
			objectKeys: new Map([[record.objectId, deliveredKey]]),
			pinnedSigners: new Map([
				[remote.deviceId, encodeBase64(remote.signingPublic)],
			]),
		});
		state.operations.push({
			workspaceId: record.workspaceId,
			operation: await codec.sealFileChange({
				workspaceId: record.workspaceId,
				objectId: record.objectId,
				epoch: 1,
				policyRevision: '1',
				change: {
					path: 'remote/from-key.md',
					previousPath: null,
					baseRevision: null,
					content: encoder.encode('# delivered key\n'),
				},
			}),
			sequence: '1',
		});

		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('synced');
		if (outcome.status === 'synced') expect(outcome.applied).toBe(1);
		expect(await files.read('remote/from-key.md')).not.toBeNull();
	});

	test('syncNow and workspaceSummary auto-load a persisted binding', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const stateStore = createMemorySyncStateStore();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const { keyStore, record } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore,
			fetch: recordingSyncFetch(state),
		});

		const reopened = await createBrowserSyncController({
			keyStore,
			origin: ORIGIN,
			fetch: recordingSyncFetch(state),
			bindingStore,
			stateStore,
			workspaceFiles: files,
		});
		const unlocked = await reopened.unlock({ passphrase: PASSPHRASE });
		expect(unlocked.ok).toBe(true);

		const summary = await reopened.workspaceSummary({
			workspaceId: 'workspace_local',
		});
		expect(summary.configured).toBe(true);
		expect(summary.workspaceId).toBe(record.workspaceId);

		const outcome = await reopened.syncNow({ workspaceId: 'workspace_local' });
		expect(outcome.status).toBe('synced');
	});

	test('resolveConflict delegates to the engine and refreshes the summary', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const stateStore = createMemorySyncStateStore();
		const files = workspaceFiles(
			{ 'notes/a.md': encoder.encode('# A local\n') },
			[{ id: 'note_a', path: 'notes/a.md' }],
		);
		const { controller, keyStore, record } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore,
			fetch: recordingSyncFetch(state),
		});
		const bundle = await keyStore.read(DEFAULT_BROWSER_SYNC_BUNDLE_ID);
		if (!bundle) throw new Error('no stored bundle');
		const identity = await unlockDeviceIdentity(bundle, PASSPHRASE);
		// The conflicting operation is authored by another device; approve its
		// signer so the engine records the conflict instead of skipping it.
		const remote = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
		const remoteRecipient = encodeRecipient(remote.x25519Public);
		state.devices = [
			{
				deviceId: remote.deviceId,
				accountId: 'acct_one',
				publicKey: encodeBase64(remote.signingPublic),
				encryptionRecipient: remoteRecipient,
			},
		];
		const approved = await controller.approveDevice(
			remote.deviceId,
			deviceFingerprintForCard(
				remote.deviceId,
				'acct_one',
				remote.signingPublic,
				remoteRecipient,
			),
		);
		if (!approved.ok) throw new Error(approved.message);

		const deliveredKey = randomBytes(32);
		await deliveredKeyFor({
			state,
			workspaceId: record.workspaceId,
			objectId: record.objectId,
			epoch: 1,
			identity,
			key: deliveredKey,
		});
		const codec = createFileChangeCodec({
			identity: remote,
			objectKeys: new Map([[record.objectId, deliveredKey]]),
			pinnedSigners: new Map([
				[remote.deviceId, encodeBase64(remote.signingPublic)],
			]),
		});
		const remoteOperation = await codec.sealFileChange({
			workspaceId: record.workspaceId,
			objectId: record.objectId,
			epoch: 1,
			policyRevision: '1',
			change: {
				path: 'notes/a.md',
				previousPath: null,
				baseRevision: null,
				content: encoder.encode('# A remote\n'),
			},
		});
		state.operations.push({
			workspaceId: record.workspaceId,
			operation: remoteOperation,
			sequence: '1',
		});

		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('synced');
		if (outcome.status === 'synced') expect(outcome.conflicts).toBe(1);

		const before = await controller.workspaceSummary();
		expect(before.conflicts).toBe(1);
		expect(before.conflictDetails[0]?.path).toBe('notes/a.md');

		const resolved = await controller.resolveConflict(
			remoteOperation.operationId,
			'remote',
		);
		if (!resolved.ok) throw new Error(resolved.message);
		expect(resolved.ok).toBe(true);
		if (resolved.ok) expect(resolved.value.remaining).toBe(0);

		const after = await controller.workspaceSummary();
		expect(after.conflicts).toBe(0);
		expect(
			new TextDecoder().decode((await files.read('notes/a.md'))?.bytes),
		).toBe('# A remote\n');
	});

	test('provisions a managed object created after bootstrap into a new policy', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const cards = [{ id: 'note_a', path: 'notes/a.md' }];
		const files = workspaceFiles(
			{ 'notes/a.md': encoder.encode('# A\n') },
			cards,
		);
		const { controller } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		expect(state.policies).toHaveLength(1);
		expect(state.accessRevision).toBe('1');

		cards.push({ id: 'note_b', path: 'notes/b.md' });
		await files.write({
			path: 'notes/b.md',
			bytes: encoder.encode('# B\n'),
			expectedRevision: null,
		});

		const outcome = await controller.syncNow();
		if (outcome.status !== 'synced') throw new Error(JSON.stringify(outcome));
		expect(state.createdObjects).toHaveLength(2);
		expect(state.policies).toHaveLength(2);
		expect(state.policies[1]?.revision).toBe('2');
		expect(state.policies[1]?.objects).toHaveLength(2);
		for (const object of state.policies[1]?.objects ?? []) {
			expect(object.envelopes).toHaveLength(1);
		}
		expect(state.accessRevision).toBe('2');

		const updated = await bindingStore.read();
		expect(Object.keys(updated?.objects ?? {})).toHaveLength(2);
		expect(state.pushes.flat().length).toBeGreaterThan(0);
	});

	test('wraps an existing object key to a newly active browser device', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const { controller, keyStore, record } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		const bundle = await keyStore.read(DEFAULT_BROWSER_SYNC_BUNDLE_ID);
		if (!bundle) throw new Error('no stored bundle');
		const identity = await unlockDeviceIdentity(bundle, PASSPHRASE);

		const remote = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
		state.devices = [
			{
				deviceId: remote.deviceId,
				accountId: 'acct_one',
				publicKey: encodeBase64(remote.signingPublic),
				encryptionRecipient: encodeRecipient(remote.x25519Public),
			},
		];

		const outcome = await controller.syncNow();
		if (outcome.status !== 'synced') throw new Error(JSON.stringify(outcome));
		expect(state.policies).toHaveLength(2);
		expect(state.policies[1]?.revision).toBe('2');
		const object = state.policies[1]?.objects?.[0];
		expect(object?.objectId).toBe(record.objectId);
		expect(object?.envelopes).toHaveLength(2);

		const remoteEnvelope = object?.envelopes.find(
			(entry) => (entry as { deviceId: string }).deviceId === remote.deviceId,
		) as {
			deviceId: string;
			wrappedKey: string;
			signature: string;
			recipientPublicKey: string;
			ephemeralPublicKey: string;
			salt: string;
			nonce: string;
		};
		expect(remoteEnvelope).toBeDefined();

		const original = await unwrapKey(
			envelopeFor(record, record.objectId),
			identity.x25519Secret,
			identity.signingPublic,
		);
		const delivered = await unwrapKey(
			{
				workspace_id: record.workspaceId,
				object_id: object!.objectId,
				epoch: 1,
				signing_device: identity.deviceId,
				device_id: remoteEnvelope.deviceId,
				recipient_public_key: remoteEnvelope.recipientPublicKey,
				ephemeral_public_key: remoteEnvelope.ephemeralPublicKey,
				salt: remoteEnvelope.salt,
				nonce: remoteEnvelope.nonce,
				wrapped_key: remoteEnvelope.wrappedKey,
				signature: remoteEnvelope.signature,
			},
			remote.x25519Secret,
			identity.signingPublic,
		);
		expect(encodeBase64(delivered)).toBe(encodeBase64(original));
	});

	test('does not upload a policy when nothing changed', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const { controller } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		expect(state.policies).toHaveLength(1);

		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('synced');
		expect(state.policies).toHaveLength(1);
		expect(state.accessRevision).toBe('1');
	});

	test('does not provision new objects while a native device is active', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const cards = [{ id: 'note_a', path: 'notes/a.md' }];
		const files = workspaceFiles(
			{ 'notes/a.md': encoder.encode('# A\n') },
			cards,
		);
		const { controller } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		state.devices = [
			{
				deviceId: 'native_device',
				accountId: 'acct_one',
				publicKey: encodeBase64(new Uint8Array(32)),
				encryptionRecipient: 'age1nativeexample',
			},
		];
		cards.push({ id: 'note_b', path: 'notes/b.md' });
		await files.write({
			path: 'notes/b.md',
			bytes: encoder.encode('# B\n'),
			expectedRevision: null,
		});

		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('synced');
		if (outcome.status === 'synced') expect(outcome.skippedUnmanaged).toBe(1);
		expect(state.createdObjects).toHaveLength(1);
		expect(state.policies).toHaveLength(1);
	});

	test('re-reads access-state and retries once on a policy revision conflict', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const cards = [{ id: 'note_a', path: 'notes/a.md' }];
		const files = workspaceFiles(
			{ 'notes/a.md': encoder.encode('# A\n') },
			cards,
		);
		const base = recordingSyncFetch(state);
		let failNextAccessPut = false;
		let conflictCount = 0;
		const fetchImpl: FetchLike = async (input, init) => {
			const pathname = new URL(requestUrl(input), ORIGIN).pathname;
			if (
				failNextAccessPut &&
				init?.method === 'PUT' &&
				/^\/v1\/workspaces\/[^/]+\/access$/.test(pathname)
			) {
				failNextAccessPut = false;
				conflictCount += 1;
				return json({ error: { code: 'sync.policy_revision_changed' } }, 409);
			}
			return base(input, init);
		};
		const { controller } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: fetchImpl,
		});

		cards.push({ id: 'note_b', path: 'notes/b.md' });
		await files.write({
			path: 'notes/b.md',
			bytes: encoder.encode('# B\n'),
			expectedRevision: null,
		});
		failNextAccessPut = true;

		const outcome = await controller.syncNow();
		if (outcome.status !== 'synced') throw new Error(JSON.stringify(outcome));
		expect(conflictCount).toBe(1);
		expect(state.policies).toHaveLength(2);
		expect(state.policies[1]?.revision).toBe('2');
		expect(state.accessRevision).toBe('2');
	});

	test('re-wraps and persists a delivered key that differs from the stored one', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const { controller, keyStore, record } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		const bundle = await keyStore.read(DEFAULT_BROWSER_SYNC_BUNDLE_ID);
		if (!bundle) throw new Error('no stored bundle');
		const identity = await unlockDeviceIdentity(bundle, PASSPHRASE);
		const before = await bindingStore.read();
		expect(before).not.toBeNull();

		const deliveredKey = randomBytes(32);
		await deliveredKeyFor({
			state,
			workspaceId: record.workspaceId,
			objectId: record.objectId,
			epoch: 1,
			identity,
			key: deliveredKey,
		});

		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('synced');
		const after = await bindingStore.read();
		expect(after?.objects[record.objectId]?.key.wrappedKey).not.toBe(
			before?.objects[record.objectId]?.key.wrappedKey,
		);

		const persisted = await unwrapKey(
			envelopeFor(after!, record.objectId),
			identity.x25519Secret,
			identity.signingPublic,
		);
		expect(encodeBase64(persisted)).toBe(encodeBase64(deliveredKey));
		expect(JSON.stringify(after)).not.toContain(encodeBase64(deliveredKey));
	});

	test('accessDigest matches a known vector', async () => {
		const policy = accessDigestVector();
		const expected =
			'a2c775ffe6e89e9b24953d68ea36ace6a77d106740b66d01de9338a65b67a7d9';
		expect(await accessDigest(policy)).toHaveLength(64);
		const { createHash } = await import('node:crypto');
		const independent = createHash('sha256')
			.update(Buffer.from(accessSigningBytes(policy)))
			.update(Buffer.from(policy.signature, 'base64'))
			.digest('hex');
		expect(await accessDigest(policy)).toBe(independent);
		expect(await accessDigest(policy)).toBe(expected);
	});
});

describe('browser sync per-object binding migration and moves', () => {
	test('loads a binding record written before per-object local ids without crashing', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const stateStore = createMemorySyncStateStore();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const { keyStore, record } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore,
			fetch: recordingSyncFetch(state),
		});

		// Simulate a record persisted before per-object local ids existed.
		const legacy = {
			...record,
			objects: Object.fromEntries(
				Object.entries(record.objects).map(([objectId, bound]) => {
					const { localObjectId: _ignored, ...rest } = bound;
					return [objectId, rest];
				}),
			),
		};
		await bindingStore.write(legacy);

		const reopened = await createBrowserSyncController({
			keyStore,
			origin: ORIGIN,
			fetch: recordingSyncFetch(state),
			bindingStore,
			stateStore,
			workspaceFiles: files,
		});
		await reopened.unlock({ passphrase: PASSPHRASE });

		const summary = await reopened.workspaceSummary({
			workspaceId: 'workspace_local',
		});
		expect(summary.configured).toBe(true);
		const outcome = await reopened.syncNow({ workspaceId: 'workspace_local' });
		expect(outcome.status).toBe('synced');
	});

	test('follows a moved managed object by its stable id', async () => {
		const state = createSyncServerState();
		const bindingStore = createMemoryBindingStore();
		const cards = [{ id: 'note_a', path: 'notes/a.md' }];
		const files = workspaceFiles(
			{ 'notes/a.md': encoder.encode('# A\n') },
			cards,
		);
		const { controller, record } = await bootstrapPerObject({
			files,
			bindingStore,
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		await controller.syncNow();

		const current = await files.read('notes/a.md');
		await files.move({
			from: 'notes/a.md',
			to: 'archive/a.md',
			expectedRevision: current?.revision ?? '',
			expectedDestinationRevision: null,
		});
		cards[0]!.path = 'archive/a.md';

		state.pushes = [];
		const outcome = await controller.syncNow();
		expect(outcome.status).toBe('synced');
		const pushed = state.pushes.flat();
		expect(pushed.length).toBeGreaterThan(0);
		for (const operation of pushed) {
			expect(operation.objectId).toBe(record.objectId);
		}
	});
});

describe('browser recovery kit controller wiring', () => {
	const OTHER_PASSPHRASE = 'definitely not the passphrase';

	test('export requires unlocked custody and a loaded binding', async () => {
		const unavailable = await createBrowserSyncController({ keyStore: null });
		const unavailableResult = await unavailable.exportRecoveryKit(PASSPHRASE);
		expect(unavailableResult.ok).toBe(false);
		if (!unavailableResult.ok)
			expect(unavailableResult.code).toBe('unavailable');

		const controller = await createBrowserSyncController({
			keyStore: createMemoryKeyStore(),
			origin: ORIGIN,
			fetch: enrollingFetch(),
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
		});
		const locked = await controller.exportRecoveryKit(PASSPHRASE);
		expect(locked.ok).toBe(false);
		if (!locked.ok) expect(locked.code).toBe('locked');

		const emptyPassphrase = await controller.exportRecoveryKit('');
		expect(emptyPassphrase.ok).toBe(false);
		if (!emptyPassphrase.ok)
			expect(emptyPassphrase.code).toBe('invalid_passphrase');

		await controller.enroll({ passphrase: PASSPHRASE });
		const noBinding = await controller.exportRecoveryKit(PASSPHRASE);
		expect(noBinding.ok).toBe(false);
		if (!noBinding.ok) expect(noBinding.code).toBe('not_configured');
	});

	test('import restores a usable binding on another browser without a trusted device', async () => {
		const state = createSyncServerState();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const source = await bootstrapPerObject({
			files,
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		const exported = await source.controller.exportRecoveryKit(PASSPHRASE);
		expect(exported.ok).toBe(true);
		if (!exported.ok) throw new Error(exported.message);

		const targetKeyStore = createMemoryKeyStore();
		const targetBindingStore = createMemoryBindingStore();
		const targetStateStore = createMemorySyncStateStore();
		const target = await createBrowserSyncController({
			keyStore: targetKeyStore,
			origin: ORIGIN,
			fetch: recordingSyncFetch(state),
			bindingStore: targetBindingStore,
			stateStore: targetStateStore,
		});

		const imported = await target.importRecoveryKit(
			exported.value,
			PASSPHRASE,
			'workspace_restored',
		);
		expect(imported.ok).toBe(true);
		if (!imported.ok) throw new Error(imported.message);
		expect(imported.value.configured).toBe(true);
		expect(imported.value.workspaceId).toBe(source.record.workspaceId);

		const stored = await targetBindingStore.read();
		expect(stored?.localWorkspaceId).toBe('workspace_restored');
		expect(stored?.workspaceId).toBe(source.record.workspaceId);
		expect(stored?.objects[source.record.objectId]?.key.wrappedKey).toBe(
			source.record.objects[source.record.objectId]?.key.wrappedKey,
		);

		const unlocked = await target.unlock({ passphrase: PASSPHRASE });
		expect(unlocked.ok).toBe(true);
		target.setWorkspaceFiles(files);
		const bound = await target.bindWorkspace({
			workspaceId: 'workspace_restored',
			workspaceFiles: files,
		});
		expect(bound.ok).toBe(true);
		if (bound.ok) expect(bound.value.configured).toBe(true);
	});

	test('import rejects a wrong passphrase and writes nothing', async () => {
		const state = createSyncServerState();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const source = await bootstrapPerObject({
			files,
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		const exported = await source.controller.exportRecoveryKit(PASSPHRASE);
		if (!exported.ok) throw new Error(exported.message);

		const targetKeyStore = createMemoryKeyStore();
		const targetBindingStore = createMemoryBindingStore();
		const target = await createBrowserSyncController({
			keyStore: targetKeyStore,
			origin: ORIGIN,
			fetch: recordingSyncFetch(state),
			bindingStore: targetBindingStore,
			stateStore: createMemorySyncStateStore(),
		});

		const imported = await target.importRecoveryKit(
			exported.value,
			OTHER_PASSPHRASE,
			'workspace_restored',
		);
		expect(imported.ok).toBe(false);
		if (!imported.ok) expect(imported.code).toBe('passphrase_rejected');
		expect(
			await targetKeyStore.read(DEFAULT_BROWSER_SYNC_BUNDLE_ID),
		).toBeUndefined();
		expect(await targetBindingStore.read()).toBeNull();
	});

	test('the exported kit contains no plaintext key material', async () => {
		const state = createSyncServerState();
		const files = workspaceFiles({ 'notes/a.md': encoder.encode('# A\n') }, [
			{ id: 'note_a', path: 'notes/a.md' },
		]);
		const { controller } = await bootstrapPerObject({
			files,
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
			fetch: recordingSyncFetch(state),
		});
		const exported = await controller.exportRecoveryKit(PASSPHRASE);
		if (!exported.ok) throw new Error(exported.message);

		const serialized = JSON.stringify(exported.value);
		expect(serialized).not.toContain(PASSPHRASE);
		expect(serialized).not.toContain('signingSeed');
		expect(serialized).not.toContain('x25519Secret');
	});
});

describe('browser sync attachments', () => {
	async function attachmentRemote() {
		const workspaceId = 'ws_attachment';
		const objectId = 'obj_attachment';
		const objectKey = crypto.getRandomValues(new Uint8Array(32));
		const remoteIdentity = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
		// The receiving replica is a different device than the operation author,
		// so it does not skip the operation as its own.
		const localIdentity = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
		const plaintext = encoder.encode('attachment-bytes');
		const { ciphertext, blob } = await encryptAttachment(objectKey, plaintext);
		const payload = encodeFileChange(
			buildAttachmentFileChange({ path: 'assets/report.bin', blob }),
		);
		const operation = await sealOperation({
			objectKey,
			workspaceId,
			objectId,
			deviceId: remoteIdentity.deviceId,
			epoch: 1,
			policyRevision: '1',
			plaintext: payload,
			identity: remoteIdentity,
		});
		const remote: BrowserSyncRemote = {
			async push(operations) {
				return { sequences: operations.map((_, index) => String(index + 1)) };
			},
			async pull(cursor) {
				if (cursor === '0') {
					return {
						accessRevision: '1',
						operations: [{ ...operation, sequence: '1' }],
						cursor: '1',
						hasMore: false,
					};
				}
				return {
					accessRevision: '1',
					operations: [],
					cursor,
					hasMore: false,
				};
			},
		};
		const fetch: FetchLike = async (input, init) => {
			const url = requestUrl(input);
			const match = url.match(/\/blobs\/([0-9a-f]{64})\/content$/);
			const range = new Headers(init?.headers)
				.get('Range')
				?.match(/^bytes=(\d+)-(\d+)$/);
			if (init?.method === 'GET' && match && range) {
				const start = Number(range[1]);
				const end = Number(range[2]);
				return new Response(ciphertext.slice(start, end + 1), {
					status: 206,
					headers: {
						'Content-Range': `bytes ${start}-${end}/${ciphertext.length}`,
					},
				});
			}
			throw new Error(`unexpected attachment request in test: ${url}`);
		};
		return {
			workspaceId,
			objectId,
			objectKey,
			remoteIdentity,
			localIdentity,
			plaintext,
			blob,
			ciphertext,
			remote,
			fetch,
		};
	}

	function reconcileInput(
		setup: Awaited<ReturnType<typeof attachmentRemote>>,
		storage: MemorySyncStorage,
		withAttachments: boolean,
	) {
		return {
			identity: setup.localIdentity,
			workspaceId: setup.workspaceId,
			objectId: setup.objectId,
			epoch: 1,
			policyRevision: '1',
			objectKeys: new Map([[setup.objectId, setup.objectKey]]),
			pinnedSigners: new Map([
				[setup.remoteIdentity.deviceId, setup.remoteIdentity.signingPublic],
			]),
			storage,
			state: createMemorySyncStateStore(),
			remote: setup.remote,
			...(withAttachments
				? {
						attachments: {
							origin: ORIGIN,
							token: 'token-attachment',
							fetch: setup.fetch,
						},
					}
				: {}),
		};
	}

	test('applies a version-3 change by downloading and decrypting the blob', async () => {
		const setup = await attachmentRemote();
		const storage = new MemorySyncStorage();

		const result = await runBrowserSyncReconcile(
			reconcileInput(setup, storage, true),
		);

		expect(result.applied).toBe(1);
		const stored = await storage.read('assets/report.bin');
		expect(stored).not.toBeNull();
		expect(
			Buffer.from(stored!.bytes).equals(Buffer.from(setup.plaintext)),
		).toBe(true);
	});

	test('without transport metadata, a version-3 change is not written', async () => {
		const setup = await attachmentRemote();
		const storage = new MemorySyncStorage();

		await expect(
			runBrowserSyncReconcile(reconcileInput(setup, storage, false)),
		).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.AttachmentUnavailable,
		});
		expect(await storage.read('assets/report.bin')).toBeNull();
	});

	test('refuses an attachment larger than the browser memory bound', async () => {
		const setup = await attachmentRemote();
		const fetcher = createBrowserAttachmentFetcher({
			origin: ORIGIN,
			token: 'token-attachment',
			fetch: setup.fetch,
			workspaceId: setup.workspaceId,
			objectKeys: new Map([[setup.objectId, setup.objectKey]]),
			maxBytes: 8,
		});
		await expect(
			fetcher.fetch({
				workspaceId: setup.workspaceId,
				objectId: setup.objectId,
				epoch: 1,
				path: 'assets/report.bin',
				previousPath: null,
				baseRevision: null,
				content: null,
				blob: setup.blob,
			}),
		).rejects.toMatchObject({ code: 'browser_sync_blob_too_large' });
		expect(MAX_BROWSER_ATTACHMENT_BYTES).toBeGreaterThan(8);
	});
});

describe('browser attachment send', () => {
	const sendObjectKey = new Uint8Array(32).fill(9);
	const sendOtherKey = new Uint8Array(32).fill(11);
	const sendWorkspaceId = 'workspace';
	const sendObjectId = 'obj_note';
	const sendPath = 'attachments/obj_note/photo.png';

	function sendPattern(size: number): Uint8Array {
		const bytes = new Uint8Array(size);
		for (let index = 0; index < size; index += 1) bytes[index] = index % 251;
		return bytes;
	}

	class FakeSendRemote implements BrowserSyncRemote {
		readonly pushed: EncryptedOperation[] = [];

		async push(
			operations: EncryptedOperation[],
		): Promise<{ sequences: string[] }> {
			this.pushed.push(...operations);
			return {
				sequences: operations.map((_, index) =>
					String(this.pushed.length * 1000 + index),
				),
			};
		}

		async pull(cursor: string) {
			return {
				accessRevision: cursor,
				cursor,
				hasMore: false,
				operations: this.pushed.map((operation, index) => ({
					...operation,
					sequence: String(index),
				})) as SequencedOperation[],
			};
		}
	}

	/** Minimal tus/range blob server; no network is used. */
	class FakeBlobServer {
		readonly origin = ORIGIN;
		readonly token = 'token-attachment';
		bytes: Uint8Array | null = null;
		storedId: string | null = null;
		createBody: Record<string, unknown> | null = null;
		resumeOffset = 0;
		fail: number | null = null;
		readonly patches: number[] = [];

		readonly fetch: FetchLike = async (input, init) => {
			const url = requestUrl(input);
			const method = init?.method ?? 'GET';
			const match = url.match(/\/blobs\/([0-9a-f]{64})(\/content)?$/);
			if (method === 'POST') {
				this.createBody = JSON.parse(String(init?.body)) as Record<
					string,
					unknown
				>;
				return Response.json(
					{
						id: this.createBody.id,
						offset: this.resumeOffset,
						complete: false,
						failed: false,
					},
					{ status: 201 },
				);
			}
			if (method === 'HEAD') {
				return new Response(null, {
					status: 200,
					headers: {
						'Upload-Offset': String(this.resumeOffset),
						'Upload-Length': String(
							(this.createBody?.size as number | undefined) ??
								this.bytes?.length ??
								0,
						),
					},
				});
			}
			if (method === 'PATCH') {
				const offset = Number(new Headers(init?.headers).get('Upload-Offset'));
				const body = new Uint8Array(init?.body as ArrayBuffer);
				this.patches.push(body.length);
				if (this.fail === this.patches.length) return this.errorResponse(500);
				const total = this.createBody!.size as number;
				const next = offset + body.length;
				if (this.bytes === null) this.bytes = new Uint8Array(total);
				this.bytes.set(body, offset);
				this.resumeOffset = next;
				return new Response(null, {
					status: 204,
					headers: {
						'Upload-Offset': String(next),
						...(next === total ? { 'Noura-Blob-Complete': 'true' } : {}),
					},
				});
			}
			if (method === 'GET' && match?.[2]) {
				const id = match[1]!;
				const range = new Headers(init?.headers)
					.get('Range')
					?.match(/^bytes=(\d+)-(\d+)$/);
				if (!range || this.bytes === null || id !== this.storedId)
					return this.errorResponse(404);
				const start = Number(range[1]);
				const end = Number(range[2]);
				const slice = this.bytes.slice(start, end + 1);
				return new Response(slice, {
					status: 206,
					headers: {
						'Content-Range': `bytes ${start}-${end}/${this.bytes.length}`,
						'Content-Length': String(slice.length),
					},
				});
			}
			return this.errorResponse(404);
		};

		private errorResponse(status: number): Response {
			return Response.json(
				{ error: { code: 'sync.server_error' } },
				{ status },
			);
		}
	}

	function sendBinding(
		identity: DeviceIdentity,
		options: {
			storage?: MemorySyncStorage;
			remote?: BrowserSyncRemote;
			server?: FakeBlobServer;
			key?: Uint8Array;
		} = {},
	): BrowserSyncWorkspaceBinding {
		const server = options.server ?? new FakeBlobServer();
		return {
			workspaceId: sendWorkspaceId,
			objectId: sendObjectId,
			epoch: 1,
			policyRevision: '1',
			objectKeys: new Map([[sendObjectId, options.key ?? sendObjectKey]]),
			objects: new Map([
				[
					sendObjectId,
					{
						objectId: sendObjectId,
						path: 'Notes/a.md',
						localObjectId: 'note_a',
						epoch: 1,
						policyRevision: '1',
					},
				],
			]),
			pinnedSigners: new Map([[identity.deviceId, identity.signingPublic]]),
			storage: options.storage ?? new MemorySyncStorage(),
			state: createMemorySyncStateStore(),
			remote: options.remote ?? new FakeSendRemote(),
			attachments: {
				origin: server.origin,
				token: server.token,
				fetch: server.fetch,
			},
		};
	}

	async function sendIdentity(): Promise<DeviceIdentity> {
		return unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase: PASSPHRASE }),
			PASSPHRASE,
		);
	}

	test('encrypts, uploads, seals v3, and a fresh replica applies it', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const plaintext = sendPattern(2 * 1024 * 1024 + 123);

		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'photo.png',
			bytes: plaintext,
		});
		expect(result.path).toBe(sendPath);
		expect(server.bytes).not.toBeNull();
		expect(server.patches.every((size) => size <= 1024 * 1024)).toBe(true);

		const local = await sender.storage.read(result.path);
		expect(local).not.toBeNull();
		expect(Buffer.from(local!.bytes).equals(Buffer.from(plaintext))).toBe(true);

		const queued = await sender.state.read();
		expect(queued.outbox).toHaveLength(1);
		expect(queued.knownPaths).toContain(result.path);

		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });
		expect(remote.pushed).toHaveLength(1);
		expect(remote.pushed[0]!.objectId).toBe(sendObjectId);

		// A fresh replica with empty storage and durable state applies the queued
		// version-3 operation, fetching and decrypting the attachment.
		const receiverStorage = new MemorySyncStorage();
		const receiverIdentity = await sendIdentity();
		const receiver = sendBinding(identity, {
			storage: receiverStorage,
			remote,
			server,
		});
		const outcome = await runBrowserSyncReconcile({
			...receiver,
			identity: receiverIdentity,
		});
		expect(outcome.applied).toBe(1);
		const stored = await receiverStorage.read(result.path);
		expect(stored).not.toBeNull();
		expect(Buffer.from(stored!.bytes).equals(Buffer.from(plaintext))).toBe(
			true,
		);
	});

	test('a reloaded replica applies the persisted version-3 operation', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const plaintext = sendPattern(4096);
		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'notes.bin',
			bytes: plaintext,
		});
		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });

		// A new replica instance with empty storage reopens the encrypted
		// operation from the remote and materializes the attachment.
		const storage = new MemorySyncStorage();
		const receiverIdentity = await sendIdentity();
		const receiver = sendBinding(identity, { storage, remote, server });
		const outcome = await runBrowserSyncReconcile({
			...receiver,
			identity: receiverIdentity,
		});
		expect(outcome.applied).toBe(1);
		const stored = await storage.read(result.path);
		expect(stored).not.toBeNull();
		expect(Buffer.from(stored!.bytes).equals(Buffer.from(plaintext))).toBe(
			true,
		);
	});

	test('refuses a plaintext above the browser bound without uploading', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const sender = sendBinding(identity, { server });

		await expect(
			runBrowserSyncSendAttachment({
				...sender,
				identity,
				objectId: sendObjectId,
				name: 'big.bin',
				bytes: sendPattern(9),
				maxBytes: 8,
			}),
		).rejects.toMatchObject({ code: BrowserSyncErrorCode.BlobTooLarge });

		expect(server.createBody).toBeNull();
		const state = await sender.state.read();
		expect(state.outbox).toHaveLength(0);
		expect(
			await sender.storage.read('attachments/obj_note/big.bin'),
		).toBeNull();
	});

	test('an upload failure leaves no operation enqueued', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		server.fail = 1;
		const sender = sendBinding(identity, { server });

		await expect(
			runBrowserSyncSendAttachment({
				...sender,
				identity,
				objectId: sendObjectId,
				name: 'photo.png',
				bytes: sendPattern(4096),
			}),
		).rejects.toMatchObject({ code: BrowserSyncErrorCode.RequestFailed });

		const state = await sender.state.read();
		expect(state.outbox).toHaveLength(0);
		expect(await sender.storage.read(sendPath)).toBeNull();
	});

	test('a tampered uploaded blob is rejected and nothing is written', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'photo.png',
			bytes: sendPattern(2048),
		});
		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });
		expect(server.bytes).not.toBeNull();
		server.bytes![0] = (server.bytes![0] ?? 0) ^ 0x01;

		const receiverStorage = new MemorySyncStorage();
		const receiverIdentity = await sendIdentity();
		const receiver = sendBinding(identity, {
			storage: receiverStorage,
			remote,
			server,
		});
		await expect(
			runBrowserSyncReconcile({ ...receiver, identity: receiverIdentity }),
		).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.AttachmentUnavailable,
		});
		expect(await receiverStorage.read(result.path)).toBeNull();
	});

	test('a wrong object key cannot open the queued operation', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'photo.png',
			bytes: sendPattern(1024),
		});
		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });

		const receiverStorage = new MemorySyncStorage();
		const receiverIdentity = await sendIdentity();
		const receiver = sendBinding(identity, {
			storage: receiverStorage,
			remote,
			server,
			key: sendOtherKey,
		});
		await expect(
			runBrowserSyncReconcile({ ...receiver, identity: receiverIdentity }),
		).rejects.toMatchObject({
			code: BrowserSyncEngineErrorCode.InvalidOperation,
		});
		expect(await receiverStorage.read(result.path)).toBeNull();
	});

	test('keeps version-1 conflict rules for a version-3 change', async () => {
		const identity = await sendIdentity();
		const server = new FakeBlobServer();
		const remote = new FakeSendRemote();
		const sender = sendBinding(identity, { server, remote });
		const plaintext = sendPattern(512);
		const result = await runBrowserSyncSendAttachment({
			...sender,
			identity,
			objectId: sendObjectId,
			name: 'photo.png',
			bytes: plaintext,
		});
		server.storedId = result.blob.id;
		await runBrowserSyncReconcile({ ...sender, identity });

		const storage = new MemorySyncStorage();
		const local = new Uint8Array([1, 2, 3, 4]);
		const written = await storage.write({
			path: result.path,
			bytes: local,
			expectedRevision: null,
		});
		// Seed the baseline so the local attachment is not re-sealed; the queued
		// remote version-3 change then hits the ordinary revision rules.
		const state = createMemorySyncStateStore({
			...createEmptySyncState(),
			pushedRevisions: { [result.path]: written.revision },
			knownPaths: [result.path],
		});
		const receiver = {
			...sendBinding(identity, { storage, remote, server }),
			state,
		};
		const receiverIdentity = await sendIdentity();

		const outcome = await runBrowserSyncReconcile({
			...receiver,
			identity: receiverIdentity,
		});
		expect(outcome.applied).toBe(0);
		expect(outcome.conflicts).toHaveLength(1);
		expect(outcome.conflicts[0]!.reason).toBe('unexpected_file');
		const stored = await storage.read(result.path);
		expect(Buffer.from(stored!.bytes).equals(Buffer.from(local))).toBe(true);
	});
});

interface NativeRecoveryFixture {
	recovery: NativeRecoveryObject;
	recovery_identity: string;
	recovery_recipient: string;
	object_keys: Array<{ object_id: string; epoch: number; key: string }>;
}

const NATIVE_KIT = nativeFixture as unknown as NativeRecoveryFixture;

function nativeFlipLastByte(value: string): string {
	const bytes = decodeBase64(value);
	bytes[bytes.length - 1] = bytes[bytes.length - 1]! ^ 0x01;
	return encodeBase64(bytes);
}

function changedIdentity(identity: string): string {
	const last = identity.at(-1);
	return `${identity.slice(0, -1)}${last === '3' ? '4' : '3'}`;
}

describe('browser native recovery kit import', () => {
	async function nativeController() {
		const bindingStore = createMemoryBindingStore();
		const stateStore = createMemorySyncStateStore();
		const keyStore = createMemoryKeyStore();
		const controller = await createBrowserSyncController({
			keyStore,
			origin: ORIGIN,
			fetch: enrollingFetch('token-native'),
			bindingStore,
			stateStore,
		});
		await controller.enroll({ passphrase: PASSPHRASE });
		return { controller, keyStore, bindingStore, stateStore };
	}

	test('imports the shared fixture and persists only re-wrapped keys', async () => {
		const { controller, bindingStore } = await nativeController();
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(true);
		if (!result.ok) throw new Error(result.message);
		expect(result.value.configured).toBe(true);

		const record = await bindingStore.read();
		expect(record).not.toBeNull();
		expect(record?.localWorkspaceId).toBe('workspace_native');
		expect(record?.workspaceId).toBe(NATIVE_KIT.recovery.config.workspaceId);
		expect(record?.revision).toBe('1');
		expect(Object.keys(record!.objects).sort()).toEqual([
			'object_one',
			'object_two',
		]);
		expect(record!.objectId in record!.objects).toBe(true);

		const deviceId = controller.device()?.deviceId;
		expect(deviceId).toBeDefined();
		for (const bound of Object.values(record!.objects)) {
			expect(bound.key.construction).toBe('web');
			expect(bound.key.deviceId).toBe(deviceId!);
			expect(bound.key.wrappedKey.length).toBeGreaterThan(0);
			// A native kit carries no verified paths, so recovered objects must
			// never be treated as owners of local files.
			expect(bound.unmapped).toBe(true);
		}
	});

	test('never writes the recovery identity or a plaintext object key', async () => {
		const { controller, bindingStore } = await nativeController();
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(true);

		const serialized = JSON.stringify(await bindingStore.read());
		expect(serialized).not.toContain(NATIVE_KIT.recovery_identity);
		expect(serialized).not.toContain('recovery_identity');
		expect(serialized).not.toContain('AGE-SECRET-KEY');
		for (const vector of NATIVE_KIT.object_keys) {
			expect(serialized).not.toContain(vector.key);
		}
	});

	test('rejects replacing a binding that points at a different remote workspace', async () => {
		const { controller, bindingStore } = await nativeController();
		const boundKey = {
			deviceId: 'device_existing',
			wrappedKey: '',
			signature: '',
			construction: 'web' as const,
			recipientPublicKey: '',
			ephemeralPublicKey: '',
			salt: '',
			nonce: '',
		};
		await bindingStore.write({
			version: 1,
			localWorkspaceId: 'workspace_native',
			workspaceId: 'workspace_other',
			revision: '1',
			objectId: 'object_other',
			objects: {
				object_other: {
					path: 'notes/other.md',
					epoch: 1,
					policyRevision: '1',
					key: boundKey,
				},
			},
			pinnedSigners: { device_existing: '' },
		});
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.code).toBe('custody_failed');
		// The existing binding must be left untouched.
		expect((await bindingStore.read())?.workspaceId).toBe('workspace_other');
	});

	test('accepts a caller-pinned signer matching the kit self-description', async () => {
		const { controller, bindingStore } = await nativeController();
		const signer =
			NATIVE_KIT.recovery.config.trustedDevices[
				NATIVE_KIT.recovery.config.deviceId
			]!;
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			recoverySignerPublic: signer,
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(true);
		expect(await bindingStore.read()).not.toBeNull();
	});

	test('rejects a pinned signer that disagrees with the kit and writes nothing', async () => {
		const { controller, bindingStore } = await nativeController();
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			recoverySignerPublic: encodeBase64(new Uint8Array(32).fill(7)),
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.code).toBe('custody_failed');
		expect(await bindingStore.read()).toBeNull();
	});

	test('rejects a recovery identity that does not match the embedded one', async () => {
		const { controller, bindingStore } = await nativeController();
		const result = await controller.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: changedIdentity(NATIVE_KIT.recovery_identity),
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.code).toBe('custody_failed');
		expect(await bindingStore.read()).toBeNull();
	});

	test('rejects a tampered kit and writes nothing', async () => {
		const { controller, bindingStore } = await nativeController();

		const tamperedSignature = {
			...NATIVE_KIT,
			recovery: {
				...NATIVE_KIT.recovery,
				signature: nativeFlipLastByte(NATIVE_KIT.recovery.signature),
			},
		};
		const signatureResult = await controller.importNativeRecoveryKit(
			tamperedSignature,
			{
				recoveryIdentity: NATIVE_KIT.recovery_identity,
				localWorkspaceId: 'workspace_native',
			},
		);
		expect(signatureResult.ok).toBe(false);
		if (!signatureResult.ok)
			expect(signatureResult.code).toBe('custody_failed');

		const first = NATIVE_KIT.recovery.envelopes[0]!;
		const tamperedEnvelope = {
			...NATIVE_KIT,
			recovery: {
				...NATIVE_KIT.recovery,
				envelopes: [
					{ ...first, wrappedKey: nativeFlipLastByte(first.wrappedKey) },
					...NATIVE_KIT.recovery.envelopes.slice(1),
				],
			},
		};
		const envelopeResult = await controller.importNativeRecoveryKit(
			tamperedEnvelope,
			{
				recoveryIdentity: NATIVE_KIT.recovery_identity,
				localWorkspaceId: 'workspace_native',
			},
		);
		expect(envelopeResult.ok).toBe(false);
		expect(await bindingStore.read()).toBeNull();
	});

	test('rejects a wrong identity when the kit does not embed one', async () => {
		const { controller, bindingStore } = await nativeController();
		const { recovery_identity: _omitted, ...withoutIdentity } = NATIVE_KIT;
		const result = await controller.importNativeRecoveryKit(withoutIdentity, {
			recoveryIdentity: changedIdentity(NATIVE_KIT.recovery_identity),
			localWorkspaceId: 'workspace_native',
		});
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.code).toBe('custody_failed');
		expect(await bindingStore.read()).toBeNull();
	});

	test('returns a typed locked or unavailable result when custody is missing', async () => {
		const unavailable = await createBrowserSyncController({ keyStore: null });
		const unavailableResult = await unavailable.importNativeRecoveryKit(
			NATIVE_KIT,
			{
				recoveryIdentity: NATIVE_KIT.recovery_identity,
				localWorkspaceId: 'workspace_native',
			},
		);
		expect(unavailableResult.ok).toBe(false);
		if (!unavailableResult.ok)
			expect(unavailableResult.code).toBe('unavailable');

		const locked = await createBrowserSyncController({
			keyStore: createMemoryKeyStore(),
			origin: ORIGIN,
			fetch: enrollingFetch(),
			bindingStore: createMemoryBindingStore(),
			stateStore: createMemorySyncStateStore(),
		});
		const lockedResult = await locked.importNativeRecoveryKit(NATIVE_KIT, {
			recoveryIdentity: NATIVE_KIT.recovery_identity,
			localWorkspaceId: 'workspace_native',
		});
		expect(lockedResult.ok).toBe(false);
		if (!lockedResult.ok) expect(lockedResult.code).toBe('locked');
	});

	test('extracts the embedded recovery identity, or null when absent', () => {
		expect(extractEmbeddedRecoveryIdentity(NATIVE_KIT)).toBe(
			NATIVE_KIT.recovery_identity,
		);
		const { recovery_identity: _omitted, ...withoutIdentity } = NATIVE_KIT;
		expect(extractEmbeddedRecoveryIdentity(withoutIdentity)).toBeNull();
		expect(() =>
			extractEmbeddedRecoveryIdentity({ format: 'other' }),
		).toThrow();
	});
});
