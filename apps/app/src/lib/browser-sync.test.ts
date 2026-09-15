import { describe, expect, test } from 'bun:test';
import type { EncryptedOperation, SequencedOperation } from '@noura/shared';
import {
	createDeviceIdentity,
	createFileChangeCodec,
	createMemoryKeyStore,
	deviceFingerprintForCard,
	encodeBase64,
	encodeRecipient,
	unlockDeviceIdentity,
	wrapKey,
	type DeviceIdentity,
	type FetchLike,
} from '@noura/browser-sync';
import type { BrowserWorkspaceFiles } from '@noura/browser-workspace';
import {
	MemorySyncStorage,
	createMemorySyncStateStore,
	type BrowserSyncRemote,
	type WorkspaceStorageLike,
} from '@noura/browser-sync-engine';
import {
	DEFAULT_BROWSER_SYNC_BUNDLE_ID,
	createBrowserSyncController,
	createBrowserSyncWorkspaceBinding,
	createMemoryBindingStore,
	createSameOriginFetch,
	runBrowserSyncReconcile,
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
});

describe('browser sync reconciliation', () => {
	test('pushes local changes and applies remote operations with in-memory fakes', async () => {
		const passphrase = PASSPHRASE;
		const identity = await unlockDeviceIdentity(
			await createDeviceIdentity({ passphrase }),
			passphrase,
		);
		const objectKey = crypto.getRandomValues(new Uint8Array(32));
		const objectKeys = new Map([['obj_reconcile', objectKey]]);
		const pinnedSigners = new Map([
			[identity.deviceId, identity.signingPublic],
		]);

		const remoteCodec = createFileChangeCodec({
			identity,
			objectKeys,
			pinnedSigners: new Map([
				[identity.deviceId, encodeBase64(identity.signingPublic)],
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
		objects?: Array<{ objectId: string; envelopes: unknown[] }>;
	}>;
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
			state.policies.push(JSON.parse(String(init?.body ?? '{}')));
			return json({ ok: true });
		}
		if (pathname.endsWith('/access-state') && method === 'GET') {
			return json({
				revision: '1',
				members: [{ accountId: 'acct_one', role: 'owner' }],
				objects: [],
				envelopes: [],
				devices: state.devices,
				policy: null,
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
			identity,
			objectKeys: new Map([[record.objectId, deliveredKey]]),
			pinnedSigners: new Map([
				[identity.deviceId, encodeBase64(identity.signingPublic)],
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
			identity,
			objectKeys: new Map([[record.objectId, deliveredKey]]),
			pinnedSigners: new Map([
				[identity.deviceId, encodeBase64(identity.signingPublic)],
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
