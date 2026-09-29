import { describe, expect, test } from 'bun:test';
import {
	accessDigest,
	accessSigningBytes,
	createDeviceIdentity,
	createFileChangeCodec,
	createMemoryBindingStore,
	createMemoryKeyStore,
	deviceFingerprintForCard,
	encodeBase64,
	encodeRecipient,
	unlockDeviceIdentity,
	unwrapKey,
	type FetchLike,
} from '@noura/browser-sync';
import { createMemorySyncStateStore } from '@noura/browser-sync-engine';
import {
	createBrowserSyncController,
	DEFAULT_BROWSER_SYNC_BUNDLE_ID,
} from './controller';
import {
	accessDigestVector,
	binding,
	bootstrapPerObject,
	createSyncServerState,
	deliveredKeyFor,
	encoder,
	enrolledController,
	enrollingFetch,
	envelopeFor,
	json,
	ORIGIN,
	PASSPHRASE,
	randomBytes,
	recordingSyncFetch,
	requestUrl,
	syncFetch,
	workspaceFiles,
} from './test-support';

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
});

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
