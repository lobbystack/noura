import { describe, expect, test } from 'bun:test';
import {
	createDeviceIdentity,
	createFileChangeCodec,
	encodeBase64,
	unlockDeviceIdentity,
	type FetchLike,
} from '@noura/browser-sync';
import {
	createMemorySyncStateStore,
	MemorySyncStorage,
	type BrowserSyncRemote,
	type WorkspaceStorageLike,
} from '@noura/browser-sync-engine';
import type { EncryptedOperation, SequencedOperation } from '@noura/shared';
import { runBrowserSyncReconcile } from './reconcile';
import {
	encoder,
	enrolledController,
	json,
	ORIGIN,
	PASSPHRASE,
	requestUrl,
} from './test-support';
import { createBrowserSyncWorkspaceBinding } from './workspace-binding';

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
