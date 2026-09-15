import { describe, expect, test } from 'bun:test';
import type { EncryptedOperation, SequencedOperation } from '@noura/shared';
import {
	createDeviceIdentity,
	createFileChangeCodec,
	createMemoryKeyStore,
	encodeBase64,
	unlockDeviceIdentity,
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
		throw new Error(`unexpected sync request in test: ${method} ${url}`);
	};
}

function workspaceFiles(
	initial: Record<string, Uint8Array>,
): BrowserWorkspaceFiles {
	const memory = new MemorySyncStorage(initial);
	return {
		list: () => memory.list(),
		read: (path) => memory.read(path),
		async write(input) {
			const result = await memory.write({
				path: input.path,
				bytes: input.bytes,
				expectedRevision: input.expectedRevision,
			});
			return { path: input.path, revision: result.revision };
		},
		async move(input) {
			await memory.move({
				from: input.from,
				to: input.to,
				expectedRevision: input.expectedRevision,
			});
			const stored = await memory.read(input.to);
			return { path: input.to, revision: stored?.revision ?? '' };
		},
		delete: (input) =>
			memory.delete({
				path: input.path,
				expectedRevision: input.expectedRevision,
			}),
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
		const files = workspaceFiles({
			'notes/local.md': encoder.encode('# Local\n'),
			'.noura/workspace.yaml': encoder.encode('{"id":"workspace_local"}'),
		});

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
		const files = workspaceFiles({
			'notes/local.md': encoder.encode('# Local\n'),
		});

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
